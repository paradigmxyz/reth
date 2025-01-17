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

use core::convert::Infallible;

use alloc::{sync::Arc, vec::Vec};
use alloy_consensus::{BlockHeader, Header};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use reth_chainspec::ChainSpec;
use reth_evm::{env::EvmEnv, ConfigureEvm, ConfigureEvmEnv, NextBlockEnvAttributes};
use reth_primitives::TransactionSigned;
use reth_primitives_traits::transaction::execute::FillTxEnv;
use reth_revm::{inspector_handle_register, EvmBuilder};
use revm_primitives::{
    AnalysisKind, BlobExcessGasAndPrice, BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, Env, SpecId, TxEnv,
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

    fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
        transaction.fill_tx_env(tx_env, sender);
    }

    fn fill_tx_env_system_contract_call(
        &self,
        env: &mut Env,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) {
        #[allow(clippy::needless_update)] // side-effect of optimism fields
        let tx = TxEnv {
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
        env.tx = tx;

        // ensure the block gas limit is >= the tx
        env.block.gas_limit = U256::from(env.tx.gas_limit);

        // disable the base fee check for this call by setting the base fee to zero
        env.block.basefee = U256::ZERO;
    }

    fn fill_cfg_env(&self, cfg_env: &mut CfgEnvWithHandlerCfg, header: &Header) {
        let spec_id = config::revm_spec(self.chain_spec(), header);

        cfg_env.chain_id = self.chain_spec.chain().id();
        cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Analyse;

        cfg_env.handler_cfg.spec_id = spec_id;
    }

    fn next_cfg_and_block_env(
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
    fn evm_with_env<DB: reth_revm::Database>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        tx: TxEnv,
    ) -> reth_revm::Evm<'_, (), DB> {
        EvmBuilder::default()
            .with_db(db)
            .with_cfg_env_with_handler_cfg(evm_env.cfg_env_with_handler_cfg)
            .with_block_env(evm_env.block_env)
            .with_tx_env(tx)
            .build()
    }

    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        tx: TxEnv,
        inspector: I,
    ) -> reth_revm::Evm<'_, I, DB>
    where
        DB: reth_revm::Database,
        I: reth_revm::GetInspector<DB>,
    {
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            .with_cfg_env_with_handler_cfg(evm_env.cfg_env_with_handler_cfg)
            .with_block_env(evm_env.block_env)
            .with_tx_env(tx)
            .append_handler_register(inspector_handle_register)
            .build()
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
        let EvmEnv { cfg_env_with_handler_cfg, .. } =
            EthEvmConfig::new(Arc::new(chain_spec.clone())).cfg_and_block_env(&header);

        // Assert that the chain ID in the `cfg_env` is correctly set to the chain ID of the
        // ChainSpec
        assert_eq!(cfg_env_with_handler_cfg.chain_id, chain_spec.chain().id());
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_evm_with_env_default_spec() {
        let evm_config = EthEvmConfig::new(MAINNET.clone());

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env(db, evm_env.clone(), Default::default());

        // Check that the EVM environment
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.cfg, evm_env.cfg_env_with_handler_cfg.cfg_env);

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

        let evm_env = EvmEnv {
            cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg {
                cfg_env: cfg.clone(),
                handler_cfg: Default::default(),
            },
            ..Default::default()
        };

        let evm = evm_config.evm_with_env(db, evm_env, Default::default());

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
        let tx = TxEnv { gas_limit: 5_000_000, gas_price: U256::from(50), ..Default::default() };

        let evm_env = EvmEnv {
            cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg {
                cfg_env: CfgEnv::default(),
                handler_cfg: Default::default(),
            },
            block_env: block,
        };

        let evm = evm_config.evm_with_env(db, evm_env.clone(), tx.clone());

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.tx, tx);

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

        let handler_cfg = HandlerCfg { spec_id: SpecId::CONSTANTINOPLE, ..Default::default() };

        let evm_env = EvmEnv {
            cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg {
                cfg_env: Default::default(),
                handler_cfg,
            },
            ..Default::default()
        };

        let evm = evm_config.evm_with_env(db, evm_env, Default::default());

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

        let evm = evm_config.evm_with_env_and_inspector(
            db,
            evm_env.clone(),
            Default::default(),
            NoOpInspector,
        );

        // Check that the EVM environment is set to default values
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.cfg, evm_env.cfg_env_with_handler_cfg.cfg_env);
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
        let tx = TxEnv::default();
        let evm_env = EvmEnv {
            cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg {
                cfg_env: cfg_env.clone(),
                handler_cfg: Default::default(),
            },
            block_env: block,
        };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env, tx, NoOpInspector);

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
        let tx = TxEnv { gas_limit: 5_000_000, gas_price: U256::from(50), ..Default::default() };
        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm =
            evm_config.evm_with_env_and_inspector(db, evm_env.clone(), tx.clone(), NoOpInspector);

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.tx, tx);
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

        let handler_cfg = HandlerCfg { spec_id: SpecId::CONSTANTINOPLE, ..Default::default() };
        let evm_env = EvmEnv {
            cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg {
                handler_cfg,
                cfg_env: Default::default(),
            },
            ..Default::default()
        };

        let evm = evm_config.evm_with_env_and_inspector(
            db,
            evm_env.clone(),
            Default::default(),
            NoOpInspector,
        );

        // Check that the spec ID is set properly
        assert_eq!(evm.handler.spec_id(), SpecId::PETERSBURG);
        assert_eq!(evm.context.evm.env.block, evm_env.block_env);
        assert_eq!(evm.context.evm.env.cfg, evm_env.cfg_env_with_handler_cfg.cfg_env);
        assert_eq!(evm.context.evm.env.tx, Default::default());
        assert_eq!(evm.context.external, NoOpInspector);

        // No Optimism
        assert_eq!(
            evm.handler.cfg,
            HandlerCfg { spec_id: SpecId::PETERSBURG, ..Default::default() }
        );
    }
}
