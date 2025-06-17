use crate::{build::ScrollBlockAssembler, ScrollEvmConfig, ScrollNextBlockEnvAttributes};
use alloc::sync::Arc;
use alloy_consensus::{BlockHeader, Header};
use alloy_evm::{FromRecoveredTx, FromTxWithEncoded};
use alloy_primitives::B256;
use core::convert::Infallible;
use reth_chainspec::EthChainSpec;
use reth_evm::{ConfigureEvm, EvmEnv, ExecutionCtxFor};
use reth_primitives_traits::{
    BlockTy, NodePrimitives, SealedBlock, SealedHeader, SignedTransaction,
};
use reth_scroll_chainspec::{ChainConfig, ScrollChainConfig};
use reth_scroll_primitives::ScrollReceipt;
use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    primitives::U256,
};
use revm_scroll::ScrollSpecId;
use scroll_alloy_evm::{
    ScrollBlockExecutionCtx, ScrollBlockExecutorFactory, ScrollReceiptBuilder,
    ScrollTransactionIntoTxEnv,
};
use scroll_alloy_hardforks::ScrollHardforks;

impl<ChainSpec, N, R> ConfigureEvm for ScrollEvmConfig<ChainSpec, N, R>
where
    ChainSpec: EthChainSpec + ChainConfig<Config = ScrollChainConfig> + ScrollHardforks,
    N: NodePrimitives<
        Receipt = R::Receipt,
        SignedTx = R::Transaction,
        BlockHeader = Header,
        BlockBody = alloy_consensus::BlockBody<R::Transaction>,
        Block = alloy_consensus::Block<R::Transaction>,
    >,
    ScrollTransactionIntoTxEnv<TxEnv>:
        FromRecoveredTx<N::SignedTx> + FromTxWithEncoded<N::SignedTx>,
    R: ScrollReceiptBuilder<Receipt = ScrollReceipt, Transaction: SignedTransaction>,
    Self: Send + Sync + Unpin + Clone + 'static,
{
    type Primitives = N;
    type Error = Infallible;
    type NextBlockEnvCtx = ScrollNextBlockEnvAttributes;
    type BlockExecutorFactory = ScrollBlockExecutorFactory<R, Arc<ChainSpec>>;
    type BlockAssembler = ScrollBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &N::BlockHeader) -> EvmEnv<ScrollSpecId> {
        let chain_spec = self.chain_spec();
        let spec_id = self.spec_id_at_timestamp_and_number(header.timestamp(), header.number());

        let cfg_env = CfgEnv::<ScrollSpecId>::default()
            .with_spec(spec_id)
            .with_chain_id(chain_spec.chain().id());

        // get coinbase from chain spec
        let coinbase = if let Some(vault_address) = chain_spec.chain_config().fee_vault_address {
            vault_address
        } else {
            header.beneficiary()
        };

        let block_env = BlockEnv {
            number: header.number(),
            beneficiary: coinbase,
            timestamp: header.timestamp(),
            difficulty: header.difficulty(),
            prevrandao: header.mix_hash(),
            gas_limit: header.gas_limit(),
            basefee: header.base_fee_per_gas().unwrap_or_default(),
            // EIP-4844 excess blob gas of this block, introduced in Cancun
            blob_excess_gas_and_price: None,
        };

        EvmEnv { cfg_env, block_env }
    }

    fn next_evm_env(
        &self,
        parent: &N::BlockHeader,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnv<ScrollSpecId>, Self::Error> {
        // ensure we're not missing any timestamp based hardforks
        let spec_id =
            self.spec_id_at_timestamp_and_number(attributes.timestamp, parent.number() + 1);

        let chain_spec = self.chain_spec();

        // configure evm env based on parent block
        let cfg_env = CfgEnv::<ScrollSpecId>::default()
            .with_chain_id(chain_spec.chain().id())
            .with_spec(spec_id);

        // get coinbase from chain spec
        let coinbase = if let Some(vault_address) = chain_spec.chain_config().fee_vault_address {
            vault_address
        } else {
            attributes.suggested_fee_recipient
        };

        let block_env = BlockEnv {
            number: parent.number() + 1,
            beneficiary: coinbase,
            timestamp: attributes.timestamp,
            difficulty: U256::ONE,
            prevrandao: Some(B256::ZERO),
            gas_limit: attributes.gas_limit,
            basefee: attributes.base_fee,
            blob_excess_gas_and_price: None,
        };

        Ok(EvmEnv { cfg_env, block_env })
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> ExecutionCtxFor<'a, Self> {
        ScrollBlockExecutionCtx { parent_hash: block.header().parent_hash() }
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<N::BlockHeader>,
        _attributes: Self::NextBlockEnvCtx,
    ) -> ExecutionCtxFor<'_, Self> {
        ScrollBlockExecutionCtx { parent_hash: parent.hash() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScrollRethReceiptBuilder;
    use alloy_consensus::Header;
    use reth_chainspec::{Head, NamedChain::Scroll};
    use reth_scroll_chainspec::{ScrollChainConfig, ScrollChainSpecBuilder};
    use reth_scroll_primitives::ScrollPrimitives;
    use revm::primitives::B256;
    use revm_primitives::Address;

    #[test]
    fn test_spec_at_head() {
        let config = ScrollEvmConfig::<_, ScrollPrimitives, _>::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
            ScrollRethReceiptBuilder::default(),
        );

        // prepare all fork heads
        let curie_head = &Head { number: 7096836, ..Default::default() };
        let bernouilli_head = &Head { number: 5220340, ..Default::default() };
        let pre_bernouilli_head = &Head { number: 0, ..Default::default() };

        // check correct spec id
        assert_eq!(
            config.spec_id_at_timestamp_and_number(curie_head.timestamp, curie_head.number),
            ScrollSpecId::CURIE
        );
        assert_eq!(
            config
                .spec_id_at_timestamp_and_number(bernouilli_head.timestamp, bernouilli_head.number),
            ScrollSpecId::BERNOULLI
        );
        assert_eq!(
            config.spec_id_at_timestamp_and_number(
                pre_bernouilli_head.timestamp,
                pre_bernouilli_head.number
            ),
            ScrollSpecId::SHANGHAI
        );
    }

    #[test]
    fn test_fill_cfg_env() {
        let config = ScrollEvmConfig::<_, ScrollPrimitives, _>::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
            ScrollRethReceiptBuilder::default(),
        );

        // curie
        let curie_header = Header { number: 7096836, ..Default::default() };

        // fill cfg env
        let env = config.evm_env(&curie_header);

        // check correct cfg env
        assert_eq!(env.cfg_env.chain_id, Scroll as u64);
        assert_eq!(env.cfg_env.spec, ScrollSpecId::CURIE);

        // bernoulli
        let bernouilli_header = Header { number: 5220340, ..Default::default() };

        // fill cfg env
        let env = config.evm_env(&bernouilli_header);

        // check correct cfg env
        assert_eq!(env.cfg_env.chain_id, Scroll as u64);
        assert_eq!(env.cfg_env.spec, ScrollSpecId::BERNOULLI);

        // pre-bernoulli
        let pre_bernouilli_header = Header { number: 0, ..Default::default() };

        // fill cfg env
        let env = config.evm_env(&pre_bernouilli_header);

        // check correct cfg env
        assert_eq!(env.cfg_env.chain_id, Scroll as u64);
        assert_eq!(env.cfg_env.spec, ScrollSpecId::SHANGHAI);
    }

    #[test]
    fn test_fill_block_env() {
        let config = ScrollEvmConfig::<_, ScrollPrimitives, _>::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
            ScrollRethReceiptBuilder::default(),
        );

        // curie header
        let header = Header {
            number: 7096836,
            beneficiary: Address::random(),
            timestamp: 1719994277,
            mix_hash: B256::random(),
            base_fee_per_gas: Some(155157341),
            gas_limit: 10000000,
            ..Default::default()
        };

        // fill block env
        let env = config.evm_env(&header);

        // verify block env correctly updated
        let expected = BlockEnv {
            number: header.number,
            beneficiary: config.chain_spec().config.fee_vault_address.unwrap(),
            timestamp: header.timestamp,
            prevrandao: Some(header.mix_hash),
            difficulty: U256::ZERO,
            basefee: header.base_fee_per_gas.unwrap_or_default(),
            gas_limit: header.gas_limit,
            blob_excess_gas_and_price: None,
        };
        assert_eq!(env.block_env, expected)
    }

    #[test]
    fn test_next_cfg_and_block_env() -> eyre::Result<()> {
        let config = ScrollEvmConfig::<_, ScrollPrimitives, _>::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
            ScrollRethReceiptBuilder::default(),
        );

        // pre curie header
        let header = Header {
            number: 7096835,
            beneficiary: Address::random(),
            timestamp: 1719994274,
            mix_hash: B256::random(),
            base_fee_per_gas: None,
            gas_limit: 10000000,
            ..Default::default()
        };

        // curie block attributes
        let attributes = ScrollNextBlockEnvAttributes {
            timestamp: 1719994277,
            suggested_fee_recipient: Address::random(),
            gas_limit: 10000000,
            base_fee: 155157341,
        };

        // get next cfg env and block env
        let env = config.next_evm_env(&header, &attributes)?;
        let (cfg_env, block_env, spec) = (env.cfg_env.clone(), env.block_env, env.cfg_env.spec);

        // verify cfg env
        assert_eq!(cfg_env.chain_id, Scroll as u64);
        assert_eq!(spec, ScrollSpecId::CURIE);

        // verify block env
        let expected = BlockEnv {
            number: header.number + 1,
            beneficiary: config.chain_spec().config.fee_vault_address.unwrap(),
            timestamp: attributes.timestamp,
            prevrandao: Some(B256::ZERO),
            difficulty: U256::ONE,
            basefee: 155157341,
            gas_limit: header.gas_limit,
            blob_excess_gas_and_price: None,
        };
        assert_eq!(block_env, expected);

        Ok(())
    }
}
