use alloy_consensus::BlockHeader;
use core::fmt::Debug;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, Head};
use reth_evm::{env::EvmEnv, ConfigureEvm, ConfigureEvmEnv, Database, Evm, NextBlockEnvAttributes};
use reth_primitives::transaction::FillTxEnv;
use reth_revm::{
    inspector_handle_register,
    precompile::Bytes,
    primitives::{EVMError, ResultAndState},
    GetInspector,
};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_forks::ScrollHardfork;
use reth_scroll_primitives::ScrollTransactionSigned;
use revm::{
    precompile::Address,
    primitives::{
        AnalysisKind, BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, HandlerCfg, SpecId, TxEnv, U256,
    },
    EvmBuilder,
};
use revm_primitives::HaltReason;
use std::{convert::Infallible, sync::Arc};

/// Scroll EVM implementation.
#[derive(derive_more::Debug, derive_more::Deref, derive_more::DerefMut, derive_more::From)]
#[debug(bound(DB::Error: Debug))]
pub struct ScrollEvm<'a, EXT, DB: Database>(revm::Evm<'a, EXT, DB>);

impl<EXT, DB: Database> Evm for ScrollEvm<'_, EXT, DB> {
    type DB = DB;
    type Tx = TxEnv;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;

    fn block(&self) -> &BlockEnv {
        self.0.block()
    }

    fn transact(&mut self, tx: Self::Tx) -> Result<ResultAndState, Self::Error> {
        *self.tx_mut() = tx;
        self.0.transact()
    }

    fn transact_system_call(
        &mut self,
        _caller: Address,
        _contract: Address,
        _data: Bytes,
    ) -> Result<ResultAndState, Self::Error> {
        Err(Self::Error::Custom("Scroll does not support system calls".into()))
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        &mut self.context.evm.db
    }
}

/// Scroll EVM configuration.
#[derive(Clone, Debug)]
pub struct ScrollEvmConfig {
    /// The chain spec for Scroll.
    chain_spec: Arc<ScrollChainSpec>,
}

impl ScrollEvmConfig {
    /// Returns a new instance of [`ScrollEvmConfig`].
    pub const fn new(chain_spec: Arc<ScrollChainSpec>) -> Self {
        Self { chain_spec }
    }

    /// Returns the spec id at the given head.
    pub fn spec_id_at_head(&self, head: &Head) -> SpecId {
        let chain_spec = &self.chain_spec;
        if chain_spec.fork(ScrollHardfork::Curie).active_at_head(head) {
            SpecId::CURIE
        } else if chain_spec.fork(ScrollHardfork::Bernoulli).active_at_head(head) {
            SpecId::BERNOULLI
        } else {
            SpecId::PRE_BERNOULLI
        }
    }
}

impl ConfigureEvm for ScrollEvmConfig {
    type Evm<'a, DB: Database + 'a, I: 'a> = ScrollEvm<'a, I, DB>;
    type EvmError<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;

    fn evm_with_env<DB: Database>(&self, db: DB, evm_env: EvmEnv) -> Self::Evm<'_, DB, ()> {
        let cfg_env_with_handler_cfg = CfgEnvWithHandlerCfg {
            cfg_env: evm_env.cfg_env,
            handler_cfg: HandlerCfg { spec_id: evm_env.spec, is_scroll: true },
        };

        EvmBuilder::default()
            .with_db(db)
            .with_cfg_env_with_handler_cfg(cfg_env_with_handler_cfg)
            .with_block_env(evm_env.block_env)
            .build()
            .into()
    }

    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        inspector: I,
    ) -> Self::Evm<'_, DB, I>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        let cfg_env_with_handler_cfg = CfgEnvWithHandlerCfg {
            cfg_env: evm_env.cfg_env,
            handler_cfg: HandlerCfg { spec_id: evm_env.spec, is_scroll: true },
        };

        EvmBuilder::default()
            .with_external_context(inspector)
            .with_db(db)
            .with_cfg_env_with_handler_cfg(cfg_env_with_handler_cfg)
            .with_block_env(evm_env.block_env)
            .append_handler_register(inspector_handle_register)
            .build()
            .into()
    }
}

impl ConfigureEvmEnv for ScrollEvmConfig {
    type Header = alloy_consensus::Header;
    type Transaction = ScrollTransactionSigned;
    type Error = Infallible;
    type TxEnv = TxEnv;
    type Spec = SpecId;

    fn tx_env(&self, transaction: &Self::Transaction, signer: Address) -> Self::TxEnv {
        let mut tx_env = TxEnv::default();
        transaction.fill_tx_env(&mut tx_env, signer);
        tx_env
    }

    fn evm_env(&self, header: &Self::Header) -> EvmEnv {
        let spec_id = self.spec_id_at_head(&Head {
            number: header.number(),
            timestamp: header.timestamp(),
            difficulty: header.difficulty(),
            ..Default::default()
        });

        let mut cfg_env = CfgEnv::default();
        cfg_env.chain_id = self.chain_spec.chain().id();
        cfg_env.perf_analyse_created_bytecodes = AnalysisKind::default();

        // get coinbase from chain spec
        let coinbase = if let Some(vault_address) = self.chain_spec.config.fee_vault_address {
            vault_address
        } else {
            header.beneficiary()
        };

        let block_env = BlockEnv {
            number: U256::from(header.number()),
            coinbase,
            timestamp: U256::from(header.timestamp()),
            difficulty: if spec_id >= SpecId::MERGE { U256::ZERO } else { header.difficulty() },
            prevrandao: if spec_id >= SpecId::MERGE { header.mix_hash() } else { None },
            gas_limit: U256::from(header.gas_limit()),
            basefee: U256::from(header.base_fee_per_gas().unwrap_or_default()),
            // EIP-4844 excess blob gas of this block, introduced in Cancun
            blob_excess_gas_and_price: None,
        };

        EvmEnv { cfg_env, block_env, spec: spec_id }
    }

    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error> {
        // configure evm env based on parent block
        let cfg = CfgEnv::default().with_chain_id(self.chain_spec.chain().id());

        // ensure we're not missing any timestamp based hardforks
        let spec_id = self.spec_id_at_head(&Head {
            number: parent.number() + 1,
            timestamp: attributes.timestamp,
            ..Default::default()
        });

        // get coinbase from chain spec
        let coinbase = if let Some(vault_address) = self.chain_spec.config.fee_vault_address {
            vault_address
        } else {
            attributes.suggested_fee_recipient
        };

        let block_env = BlockEnv {
            number: U256::from(parent.number + 1),
            coinbase,
            timestamp: U256::from(attributes.timestamp),
            difficulty: U256::ZERO,
            prevrandao: Some(attributes.prev_randao),
            gas_limit: U256::from(attributes.gas_limit),
            // calculate basefee based on parent block's gas usage
            // TODO(scroll): update with correct block fee calculation for block building.
            basefee: U256::from(parent.base_fee_per_gas.unwrap_or_default()),
            blob_excess_gas_and_price: None,
        };

        let cfg_env_with_handler_cfg = CfgEnvWithHandlerCfg {
            cfg_env: cfg,
            handler_cfg: HandlerCfg { spec_id, is_scroll: true },
        };

        Ok((cfg_env_with_handler_cfg, block_env).into())
    }
}

pub(crate) trait ScrollConfigureEvm: ConfigureEvm {
    type Evm<'a, DB: Database + 'a, I: 'a>: Evm<Tx = Self::TxEnv, DB = DB, Error = EVMError<DB::Error>>
        + ScrollEvmT;

    fn scroll_evm_for_block<'a, DB: Database>(
        &'a self,
        db: DB,
        header: &'a Self::Header,
    ) -> <Self as ScrollConfigureEvm>::Evm<'a, DB, ()>;
}

impl ScrollConfigureEvm for ScrollEvmConfig {
    type Evm<'a, DB: Database + 'a, I: 'a> = ScrollEvm<'a, (), DB>;

    fn scroll_evm_for_block<'a, DB: Database>(
        &'a self,
        db: DB,
        header: &'a Self::Header,
    ) -> <Self as ScrollConfigureEvm>::Evm<'a, DB, ()> {
        self.evm_for_block(db, header)
    }
}

pub(crate) trait ScrollEvmT {
    /// Sets whether the evm should enable or disable the base fee checks.
    fn with_base_fee_check(&mut self, enabled: bool);
    /// Returns the l1 fee for the transaction.
    fn l1_fee(&self) -> Option<U256>;
}

impl<DB> ScrollEvmT for ScrollEvm<'_, (), DB>
where
    DB: Database,
{
    fn with_base_fee_check(&mut self, enabled: bool) {
        self.0.context.evm.inner.env.cfg.disable_base_fee = !enabled;
    }

    fn l1_fee(&self) -> Option<U256> {
        let l1_block_info = self.0.context.evm.inner.l1_block_info.as_ref()?;
        let transaction_rlp_bytes = self.0.context.evm.env.tx.scroll.rlp_bytes.as_ref()?;
        Some(l1_block_info.calculate_tx_l1_cost(transaction_rlp_bytes, self.handler.cfg.spec_id))
    }
}

impl ChainSpecProvider for ScrollEvmConfig {
    type ChainSpec = ScrollChainSpec;

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
    use revm::primitives::{SpecId, B256};

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
        assert_eq!(config.spec_id_at_head(curie_head), SpecId::CURIE);
        assert_eq!(config.spec_id_at_head(bernouilli_head), SpecId::BERNOULLI);
        assert_eq!(config.spec_id_at_head(pre_bernouilli_head), SpecId::PRE_BERNOULLI);
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
        assert_eq!(env.cfg_env.perf_analyse_created_bytecodes, AnalysisKind::Analyse);
        assert_eq!(env.spec, SpecId::CURIE);

        // bernoulli
        let bernouilli_header = Header { number: 5220340, ..Default::default() };

        // fill cfg env
        let env = config.evm_env(&bernouilli_header);

        // check correct cfg env
        assert_eq!(env.cfg_env.chain_id, Scroll as u64);
        assert_eq!(env.cfg_env.perf_analyse_created_bytecodes, AnalysisKind::Analyse);
        assert_eq!(env.spec, SpecId::BERNOULLI);

        // pre-bernoulli
        let pre_bernouilli_header = Header { number: 0, ..Default::default() };

        // fill cfg env
        let env = config.evm_env(&pre_bernouilli_header);

        // check correct cfg env
        assert_eq!(env.cfg_env.chain_id, Scroll as u64);
        assert_eq!(env.cfg_env.perf_analyse_created_bytecodes, AnalysisKind::Analyse);
        assert_eq!(env.spec, SpecId::PRE_BERNOULLI);
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
            number: U256::from(header.number),
            coinbase: config.chain_spec.config.fee_vault_address.unwrap(),
            timestamp: U256::from(header.timestamp),
            prevrandao: Some(header.mix_hash),
            difficulty: U256::ZERO,
            basefee: U256::from(header.base_fee_per_gas.unwrap_or_default()),
            gas_limit: U256::from(header.gas_limit),
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
        let (cfg_env, block_env, spec) = (env.cfg_env, env.block_env, env.spec);

        // verify cfg env
        assert_eq!(cfg_env.chain_id, Scroll as u64);
        assert_eq!(spec, SpecId::CURIE);

        // verify block env
        let expected = BlockEnv {
            number: U256::from(header.number + 1),
            coinbase: config.chain_spec.config.fee_vault_address.unwrap(),
            timestamp: U256::from(attributes.timestamp),
            prevrandao: Some(attributes.prev_randao),
            difficulty: U256::ZERO,
            // TODO(scroll): this shouldn't be 0 at curie fork
            basefee: U256::ZERO,
            gas_limit: U256::from(header.gas_limit),
            blob_excess_gas_and_price: None,
        };
        assert_eq!(block_env, expected);

        Ok(())
    }
}
