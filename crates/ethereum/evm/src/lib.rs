//! EVM config for vanilla ethereum.
//!
//! # Revm features
//!
//! This crate does __not__ enforce specific revm features such as `blst` or `c-kzg`, which are
//! critical for revm's evm internals, it is the responsibility of the implementer to ensure the
//! proper features are selected.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{sync::Arc, vec::Vec};
use alloy_consensus::{BlockHeader, Header};
use alloy_primitives::{Address, U256};
use core::{convert::Infallible, fmt::Debug};
use reth_chainspec::ChainSpec;
use reth_evm::{env::EvmEnv, ConfigureEvm, ConfigureEvmEnv, Database, Evm, NextBlockEnvAttributes};
use reth_primitives::TransactionSigned;
use reth_primitives_traits::transaction::execute::FillTxEnv;
use reth_revm::{inspector_handle_register, EvmBuilder};
use revm_primitives::{
    AnalysisKind, BlobExcessGasAndPrice, BlockEnv, Bytes, CfgEnv, CfgEnvWithHandlerCfg, EVMError,
    HaltReason, HandlerCfg, ResultAndState, SpecId, TxEnv, TxKind,
};

mod config;
use alloy_eips::{eip1559::INITIAL_BASE_FEE, eip7840::BlobParams};
pub use config::{revm_spec, revm_spec_by_timestamp_and_block_number};
use reth_ethereum_forks::EthereumHardfork;

pub mod execute;

/// Ethereum DAO hardfork state change data.
pub mod dao_fork;

/// [EIP-6110](https://eips.ethereum.org/EIPS/eip-6110) handling.
pub mod eip6110;

/// Ethereum EVM implementation.
#[derive(derive_more::Debug, derive_more::Deref, derive_more::DerefMut, derive_more::From)]
#[debug(bound(DB::Error: Debug))]
pub struct EthEvm<'a, EXT, DB: Database>(reth_revm::Evm<'a, EXT, DB>);

impl<EXT, DB: Database> Evm for EthEvm<'_, EXT, DB> {
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
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState, Self::Error> {
        #[allow(clippy::needless_update)] // side-effect of optimism fields
        let tx_env = TxEnv {
            caller,
            transact_to: TxKind::Call(contract),
            // Explicitly set nonce to None so revm does not do any nonce checks
            nonce: None,
            gas_limit: 30_000_000,
            value: U256::ZERO,
            data,
            // Setting the gas price to zero enforces that no value is transferred as part of the
            // call, and that the call will not count against the block's gas limit
            gas_price: U256::ZERO,
            // The chain ID check is not relevant here and is disabled if set to None
            chain_id: None,
            // Setting the gas priority fee to None ensures the effective gas price is derived from
            // the `gas_price` field, which we need to be zero
            gas_priority_fee: None,
            access_list: Vec::new(),
            // blob fields can be None for this tx
            blob_hashes: Vec::new(),
            max_fee_per_blob_gas: None,
            // TODO remove this once this crate is no longer built with optimism
            ..Default::default()
        };

        *self.tx_mut() = tx_env;

        let prev_block_env = self.block().clone();

        // ensure the block gas limit is >= the tx
        self.block_mut().gas_limit = U256::from(self.tx().gas_limit);

        // disable the base fee check for this call by setting the base fee to zero
        self.block_mut().basefee = U256::ZERO;

        let res = self.0.transact();

        // re-set the block env
        *self.block_mut() = prev_block_env;

        res
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        &mut self.context.evm.db
    }
}

/// Ethereum-related EVM configuration.
#[derive(Debug, Clone)]
pub struct EthEvmConfig {
    chain_spec: Arc<ChainSpec>,
}

impl EthEvmConfig {
    /// Creates a new Ethereum EVM configuration with the given chain spec.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }
}

impl ConfigureEvmEnv for EthEvmConfig {
    type Header = Header;
    type Transaction = TransactionSigned;
    type Error = Infallible;
    type TxEnv = TxEnv;
    type Spec = revm_primitives::SpecId;

    fn tx_env(&self, transaction: &TransactionSigned, sender: Address) -> Self::TxEnv {
        let mut tx_env = TxEnv::default();
        transaction.fill_tx_env(&mut tx_env, sender);
        tx_env
    }

    fn evm_env(&self, header: &Self::Header) -> EvmEnv {
        let spec = config::revm_spec(self.chain_spec(), header);

        let mut cfg_env = CfgEnv::default();
        cfg_env.chain_id = self.chain_spec.chain().id();
        cfg_env.perf_analyse_created_bytecodes = AnalysisKind::default();

        let block_env = BlockEnv {
            number: U256::from(header.number()),
            coinbase: header.beneficiary(),
            timestamp: U256::from(header.timestamp()),
            difficulty: if spec >= SpecId::MERGE { U256::ZERO } else { header.difficulty() },
            prevrandao: if spec >= SpecId::MERGE { header.mix_hash() } else { None },
            gas_limit: U256::from(header.gas_limit()),
            basefee: U256::from(header.base_fee_per_gas().unwrap_or_default()),
            // EIP-4844 excess blob gas of this block, introduced in Cancun
            blob_excess_gas_and_price: header.excess_blob_gas.map(|excess_blob_gas| {
                BlobExcessGasAndPrice::new(excess_blob_gas, spec >= SpecId::PRAGUE)
            }),
        };

        EvmEnv { cfg_env, spec, block_env }
    }

    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error> {
        // configure evm env based on parent block
        let cfg = CfgEnv::default().with_chain_id(self.chain_spec.chain().id());

        // ensure we're not missing any timestamp based hardforks
        let spec_id = revm_spec_by_timestamp_and_block_number(
            &self.chain_spec,
            attributes.timestamp,
            parent.number() + 1,
        );

        let blob_params =
            if spec_id >= SpecId::PRAGUE { BlobParams::prague() } else { BlobParams::cancun() };

        // if the parent block did not have excess blob gas (i.e. it was pre-cancun), but it is
        // cancun now, we need to set the excess blob gas to the default value(0)
        let blob_excess_gas_and_price = parent
            .next_block_excess_blob_gas(blob_params)
            .or_else(|| (spec_id == SpecId::CANCUN).then_some(0))
            .map(|gas| BlobExcessGasAndPrice::new(gas, spec_id >= SpecId::PRAGUE));

        let mut basefee = parent.next_block_base_fee(
            self.chain_spec.base_fee_params_at_timestamp(attributes.timestamp),
        );

        let mut gas_limit = U256::from(attributes.gas_limit);

        // If we are on the London fork boundary, we need to multiply the parent's gas limit by the
        // elasticity multiplier to get the new gas limit.
        if self.chain_spec.fork(EthereumHardfork::London).transitions_at_block(parent.number + 1) {
            let elasticity_multiplier = self
                .chain_spec
                .base_fee_params_at_timestamp(attributes.timestamp)
                .elasticity_multiplier;

            // multiply the gas limit by the elasticity multiplier
            gas_limit *= U256::from(elasticity_multiplier);

            // set the base fee to the initial base fee from the EIP-1559 spec
            basefee = Some(INITIAL_BASE_FEE)
        }

        let block_env = BlockEnv {
            number: U256::from(parent.number + 1),
            coinbase: attributes.suggested_fee_recipient,
            timestamp: U256::from(attributes.timestamp),
            difficulty: U256::ZERO,
            prevrandao: Some(attributes.prev_randao),
            gas_limit,
            // calculate basefee based on parent block's gas usage
            basefee: basefee.map(U256::from).unwrap_or_default(),
            // calculate excess gas based on parent block's blob gas usage
            blob_excess_gas_and_price,
        };

        Ok((CfgEnvWithHandlerCfg::new_with_spec_id(cfg, spec_id), block_env).into())
    }
}

impl ConfigureEvm for EthEvmConfig {
    type Evm<'a, DB: Database + 'a, I: 'a> = EthEvm<'a, I, DB>;
    type EvmError<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;

    fn evm_with_env<DB: Database>(&self, db: DB, evm_env: EvmEnv) -> Self::Evm<'_, DB, ()> {
        let cfg_env_with_handler_cfg = CfgEnvWithHandlerCfg {
            cfg_env: evm_env.cfg_env,
            handler_cfg: HandlerCfg::new(evm_env.spec),
        };
        EthEvm(
            EvmBuilder::default()
                .with_db(db)
                .with_cfg_env_with_handler_cfg(cfg_env_with_handler_cfg)
                .with_block_env(evm_env.block_env)
                .build(),
        )
    }

    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        inspector: I,
    ) -> Self::Evm<'_, DB, I>
    where
        DB: Database,
        I: reth_revm::GetInspector<DB>,
    {
        let cfg_env_with_handler_cfg = CfgEnvWithHandlerCfg {
            cfg_env: evm_env.cfg_env,
            handler_cfg: HandlerCfg::new(evm_env.spec),
        };
        EthEvm(
            EvmBuilder::default()
                .with_db(db)
                .with_external_context(inspector)
                .with_cfg_env_with_handler_cfg(cfg_env_with_handler_cfg)
                .with_block_env(evm_env.block_env)
                .append_handler_register(inspector_handle_register)
                .build(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_genesis::Genesis;
    use alloy_primitives::U256;
    use reth_chainspec::{Chain, ChainSpec, MAINNET};
    use reth_evm::{env::EvmEnv, execute::ProviderError};
    use reth_revm::{
        db::{CacheDB, EmptyDBTyped},
        inspectors::NoOpInspector,
        primitives::{BlockEnv, CfgEnv, SpecId},
    };
    use revm_primitives::HandlerCfg;

    #[test]
    fn test_fill_cfg_and_block_env() {
        // Create a default header
        let header = Header::default();

        // Build the ChainSpec for Ethereum mainnet, activating London, Paris, and Shanghai
        // hardforks
        let chain_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .london_activated()
            .paris_activated()
            .shanghai_activated()
            .build();

        // Use the `EthEvmConfig` to fill the `cfg_env` and `block_env` based on the ChainSpec,
        // Header, and total difficulty
        let EvmEnv { cfg_env, .. } =
            EthEvmConfig::new(Arc::new(chain_spec.clone())).evm_env(&header);

        // Assert that the chain ID in the `cfg_env` is correctly set to the chain ID of the
        // ChainSpec
        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_default_spec() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Check that the EVM environment
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.cfg, evm_env.cfg_env);

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // No Optimism
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, ..Default::default() });
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_custom_cfg() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create a custom configuration environment with a chain ID of 111
        let cfg = CfgEnv::default().with_chain_id(111);

        let evm_env = EvmEnv { cfg_env: cfg.clone(), ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env);

        // Check that the EVM environment is initialized with the custom environment
        assert_eq!(evm.context.evm.inner.env.cfg, cfg);

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // No Optimism
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, ..Default::default() });
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_custom_block_and_tx() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create customs block and tx env
        let block = BlockEnv {
            basefee: U256::from(1000),
            gas_limit: U256::from(10_000_000),
            number: U256::from(42),
            ..Default::default()
        };

        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // No Optimism
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, ..Default::default() });
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_spec_id() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv { spec: SpecId::CONSTANTINOPLE, ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env);

        // Check that the spec ID is setup properly
        assert_eq!(evm.handler.spec_id(), SpecId::PETERSBURG);

        // No Optimism
        assert_eq!(
            evm.handler.cfg,
            HandlerCfg { spec_id: SpecId::PETERSBURG, ..Default::default() }
        );
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_and_default_inspector() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector);

        // Check that the EVM environment is set to default values
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.cfg, evm_env.cfg_env);
        assert_eq!(evm.context.external, NoOpInspector);
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // No Optimism
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, ..Default::default() });
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_inspector_and_custom_cfg() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let cfg_env = CfgEnv::default().with_chain_id(111);
        let block = BlockEnv::default();
        let evm_env =
            EvmEnv { cfg_env: cfg_env.clone(), block_env: block, spec: Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env, NoOpInspector);

        // Check that the EVM environment is set with custom configuration
        assert_eq!(evm.context.evm.env.cfg, cfg_env);
        assert_eq!(evm.context.external, NoOpInspector);
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // No Optimism
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, ..Default::default() });
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_inspector_and_custom_block_tx() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create custom block and tx environment
        let block = BlockEnv {
            basefee: U256::from(1000),
            gas_limit: U256::from(10_000_000),
            number: U256::from(42),
            ..Default::default()
        };
        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector);

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.external, NoOpInspector);
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // No Optimism
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, ..Default::default() });
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_inspector_and_spec_id() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv { spec: SpecId::CONSTANTINOPLE, ..Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector);

        // Check that the spec ID is set properly
        assert_eq!(evm.handler.spec_id(), SpecId::PETERSBURG);
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.cfg, evm_env.cfg_env);
        assert_eq!(evm.context.evm.env.tx, Default::default());
        assert_eq!(evm.context.external, NoOpInspector);

        // No Optimism
        assert_eq!(
            evm.handler.cfg,
            HandlerCfg { spec_id: SpecId::PETERSBURG, ..Default::default() }
        );
    }
}
