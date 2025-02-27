use alloy_consensus::BlockHeader;
use core::fmt::Debug;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, Head};
use reth_evm::{
    ConfigureEvm, ConfigureEvmEnv, Database, Evm, EvmEnv, EvmErrorFor, HaltReasonFor,
    NextBlockEnvAttributes,
};
use reth_revm::Inspector;
use reth_scroll_chainspec::{ChainConfig, ScrollChainConfig, ScrollChainSpec};
use reth_scroll_forks::{ScrollHardfork, ScrollHardforks};
use reth_scroll_primitives::ScrollTransactionSigned;
use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    inspector::NoOpInspector,
    primitives::U256,
};
use revm_scroll::{builder::ScrollContext, ScrollSpecId};
use scroll_alloy_evm::{ScrollEvm, ScrollEvmFactory, ScrollTransactionIntoTxEnv};
use std::{convert::Infallible, sync::Arc};

/// Scroll EVM configuration.
#[derive(Clone, Debug)]
pub struct ScrollEvmConfig<ChainSpec = ScrollChainSpec> {
    /// The chain spec for Scroll.
    chain_spec: Arc<ChainSpec>,
    /// The Scroll evm factory.
    evm_factory: ScrollEvmFactory,
}

impl<ChainSpec: ScrollHardforks> ScrollEvmConfig<ChainSpec> {
    /// Returns a new instance of [`ScrollEvmConfig`].
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec, evm_factory: ScrollEvmFactory::default() }
    }

    /// Returns the chain specification for the EVM config.
    pub fn chain_spec(&self) -> &ChainSpec {
        &self.chain_spec
    }

    /// Returns the spec id at the given head.
    pub fn spec_id_at_head(&self, head: &Head) -> ScrollSpecId {
        let chain_spec = &self.chain_spec;
        if chain_spec.scroll_fork_activation(ScrollHardfork::DarwinV2).active_at_head(head) ||
            chain_spec.scroll_fork_activation(ScrollHardfork::Darwin).active_at_head(head)
        {
            ScrollSpecId::DARWIN
        } else if chain_spec.scroll_fork_activation(ScrollHardfork::Curie).active_at_head(head) {
            ScrollSpecId::CURIE
        } else {
            ScrollSpecId::BERNOULLI
        }
    }
}

impl<ChainSpec: EthChainSpec + ChainConfig<Config = ScrollChainConfig> + ScrollHardforks>
    ConfigureEvm for ScrollEvmConfig<ChainSpec>
{
    type EvmFactory = ScrollEvmFactory;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }
}

impl<ChainSpec: EthChainSpec + ChainConfig<Config = ScrollChainConfig> + ScrollHardforks>
    ConfigureEvmEnv for ScrollEvmConfig<ChainSpec>
{
    type Header = alloy_consensus::Header;
    type Transaction = ScrollTransactionSigned;
    type TxEnv = ScrollTransactionIntoTxEnv<TxEnv>;
    type Error = Infallible;
    type Spec = ScrollSpecId;

    fn evm_env(&self, header: &Self::Header) -> EvmEnv<Self::Spec> {
        let spec_id = self.spec_id_at_head(&Head {
            number: header.number(),
            timestamp: header.timestamp(),
            difficulty: header.difficulty(),
            ..Default::default()
        });

        let cfg_env = CfgEnv::<ScrollSpecId>::default()
            .with_spec(spec_id)
            .with_chain_id(self.chain_spec.chain().id());

        // get coinbase from chain spec
        let coinbase = if let Some(vault_address) = self.chain_spec.chain_config().fee_vault_address
        {
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
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes,
    ) -> Result<EvmEnv<Self::Spec>, Self::Error> {
        // ensure we're not missing any timestamp based hardforks
        let spec_id = self.spec_id_at_head(&Head {
            number: parent.number() + 1,
            timestamp: attributes.timestamp,
            ..Default::default()
        });

        // configure evm env based on parent block
        let cfg_env = CfgEnv::<ScrollSpecId>::default()
            .with_chain_id(self.chain_spec.chain().id())
            .with_spec(spec_id);

        // get coinbase from chain spec
        let coinbase = if let Some(vault_address) = self.chain_spec.chain_config().fee_vault_address
        {
            vault_address
        } else {
            attributes.suggested_fee_recipient
        };

        let block_env = BlockEnv {
            number: parent.number + 1,
            beneficiary: coinbase,
            timestamp: attributes.timestamp,
            difficulty: U256::ZERO,
            prevrandao: Some(attributes.prev_randao),
            gas_limit: attributes.gas_limit,
            // calculate basefee based on parent block's gas usage
            // TODO(scroll): update with correct block fee calculation for block building.
            basefee: parent.base_fee_per_gas.unwrap_or_default(),
            blob_excess_gas_and_price: None,
        };

        Ok(EvmEnv { cfg_env, block_env })
    }
}

pub(crate) trait ScrollConfigureEvm: ConfigureEvm {
    type ScrollEvm<DB: Database>: Evm<
            DB = DB,
            Tx = Self::TxEnv,
            HaltReason = HaltReasonFor<Self>,
            Error = EvmErrorFor<Self, DB::Error>,
        > + ScrollEvmT;

    fn scroll_evm_for_block<DB: Database>(
        &self,
        db: DB,
        header: &Self::Header,
    ) -> Self::ScrollEvm<DB>;
}

impl<ChainSpec: EthChainSpec + ChainConfig<Config = ScrollChainConfig> + ScrollHardforks>
    ScrollConfigureEvm for ScrollEvmConfig<ChainSpec>
{
    type ScrollEvm<DB: Database> = ScrollEvm<DB, NoOpInspector>;

    fn scroll_evm_for_block<DB: Database>(
        &self,
        db: DB,
        header: &Self::Header,
    ) -> Self::ScrollEvm<DB> {
        self.evm_for_block(db, header)
    }
}

pub(crate) trait ScrollEvmT: Evm {
    /// Sets whether the evm should enable or disable the base fee checks.
    fn with_base_fee_check(&mut self, enabled: bool);
    /// Returns the l1 fee for the transaction.
    fn l1_fee(&self) -> Option<U256>;
}

impl<DB, I> ScrollEvmT for ScrollEvm<DB, I>
where
    DB: Database,
    I: Inspector<ScrollContext<DB>>,
{
    fn with_base_fee_check(&mut self, enabled: bool) {
        self.ctx_mut().cfg.disable_base_fee = !enabled;
    }

    fn l1_fee(&self) -> Option<U256> {
        let l1_block_info = &self.ctx().chain;
        let transaction_rlp_bytes = self.ctx().tx.rlp_bytes.as_ref()?;
        Some(l1_block_info.calculate_tx_l1_cost(transaction_rlp_bytes, self.ctx().cfg.spec))
    }
}

impl<ChainSpec: EthChainSpec + Send + Sync + 'static> ChainSpecProvider
    for ScrollEvmConfig<ChainSpec>
{
    type ChainSpec = ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use reth_chainspec::NamedChain::Scroll;
    use reth_scroll_chainspec::{ScrollChainConfig, ScrollChainSpecBuilder};
    use revm::primitives::B256;
    use revm_primitives::Address;

    #[test]
    fn test_spec_at_head() {
        let config = ScrollEvmConfig::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
        );

        // prepare all fork heads
        let curie_head = &Head { number: 7096836, ..Default::default() };
        let bernouilli_head = &Head { number: 5220340, ..Default::default() };
        let pre_bernouilli_head = &Head { number: 0, ..Default::default() };

        // check correct spec id
        assert_eq!(config.spec_id_at_head(curie_head), ScrollSpecId::CURIE);
        assert_eq!(config.spec_id_at_head(bernouilli_head), ScrollSpecId::BERNOULLI);
        assert_eq!(config.spec_id_at_head(pre_bernouilli_head), ScrollSpecId::BERNOULLI);
    }

    #[test]
    fn test_fill_cfg_env() {
        let config = ScrollEvmConfig::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
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
        assert_eq!(env.cfg_env.spec, ScrollSpecId::BERNOULLI);
    }

    #[test]
    fn test_fill_block_env() {
        let config = ScrollEvmConfig::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
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
            beneficiary: config.chain_spec.config.fee_vault_address.unwrap(),
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
        let config = ScrollEvmConfig::new(
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()).into(),
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
        let attributes = NextBlockEnvAttributes {
            timestamp: 1719994277,
            suggested_fee_recipient: Address::random(),
            prev_randao: B256::random(),
            gas_limit: 10000000,
        };

        // get next cfg env and block env
        let env = config.next_evm_env(&header, attributes)?;
        let (cfg_env, block_env, spec) = (env.cfg_env.clone(), env.block_env, env.cfg_env.spec);

        // verify cfg env
        assert_eq!(cfg_env.chain_id, Scroll as u64);
        assert_eq!(spec, ScrollSpecId::CURIE);

        // verify block env
        let expected = BlockEnv {
            number: header.number + 1,
            beneficiary: config.chain_spec.config.fee_vault_address.unwrap(),
            timestamp: attributes.timestamp,
            prevrandao: Some(attributes.prev_randao),
            difficulty: U256::ZERO,
            // TODO(scroll): this shouldn't be 0 at curie fork
            basefee: 0,
            gas_limit: header.gas_limit,
            blob_excess_gas_and_price: None,
        };
        assert_eq!(block_env, expected);

        Ok(())
    }
}
