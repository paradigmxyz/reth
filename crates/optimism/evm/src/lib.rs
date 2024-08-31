//! EVM config for vanilla optimism.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

use reth_chainspec::ChainSpec;
use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{
    revm_primitives::{AnalysisKind, CfgEnvWithHandlerCfg, TxEnv},
    transaction::FillTxEnv,
    Address, Head, Header, TransactionSigned, U256,
};
use reth_revm::{inspector_handle_register, Database, Evm, EvmBuilder, GetInspector};

mod config;
pub use config::{revm_spec, revm_spec_by_timestamp_after_bedrock};
mod execute;
pub use execute::*;
pub mod l1;
pub use l1::*;

mod error;
pub use error::OptimismBlockExecutionError;
use revm_primitives::{Bytes, Env, OptimismFields, TxKind};

/// Optimism-related EVM configuration.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct OptimismEvmConfig;

impl ConfigureEvmEnv for OptimismEvmConfig {
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
        env.tx = TxEnv {
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
            authorization_list: None,
            optimism: OptimismFields {
                source_hash: None,
                mint: None,
                is_system_transaction: Some(false),
                // The L1 fee is not charged for the EIP-4788 transaction, submit zero bytes for the
                // enveloped tx size.
                enveloped_tx: Some(Bytes::default()),
            },
        };

        // ensure the block gas limit is >= the tx
        env.block.gas_limit = U256::from(env.tx.gas_limit);

        // disable the base fee check for this call by setting the base fee to zero
        env.block.basefee = U256::ZERO;
    }

    fn fill_cfg_env(
        &self,
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        let spec_id = revm_spec(
            chain_spec,
            &Head {
                number: header.number,
                timestamp: header.timestamp,
                difficulty: header.difficulty,
                total_difficulty,
                hash: Default::default(),
            },
        );

        cfg_env.chain_id = chain_spec.chain().id();
        cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Analyse;

        cfg_env.handler_cfg.spec_id = spec_id;
        cfg_env.handler_cfg.is_optimism = chain_spec.is_optimism();
    }
}

impl ConfigureEvm for OptimismEvmConfig {
    type DefaultExternalContext<'a> = ();

    fn evm<DB: Database>(&self, db: DB) -> Evm<'_, Self::DefaultExternalContext<'_>, DB> {
        EvmBuilder::default().with_db(db).optimism().build()
    }

    fn evm_with_inspector<DB, I>(&self, db: DB, inspector: I) -> Evm<'_, I, DB>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            .optimism()
            .append_handler_register(inspector_handle_register)
            .build()
    }

    fn default_external_context<'a>(&self) -> Self::DefaultExternalContext<'a> {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::{Chain, ChainSpec};
    use reth_evm::execute::ProviderError;
    use reth_primitives::{
        revm_primitives::{BlockEnv, CfgEnv, SpecId},
        Genesis, Header, B256, KECCAK_EMPTY, U256,
    };
    use reth_revm::{
        db::{CacheDB, EmptyDBTyped},
        inspectors::NoOpInspector,
        JournaledState,
    };
    use revm_primitives::{CfgEnvWithHandlerCfg, EnvWithHandlerCfg, HandlerCfg};
    use std::collections::HashSet;

    #[test]
    fn test_fill_cfg_and_block_env() {
        // Create a new configuration environment
        let mut cfg_env = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);

        // Create a default block environment
        let mut block_env = BlockEnv::default();

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

        // Define the total difficulty as zero (default)
        let total_difficulty = U256::ZERO;

        // Use the `OptimismEvmConfig` to fill the `cfg_env` and `block_env` based on the ChainSpec,
        // Header, and total difficulty
        OptimismEvmConfig::default().fill_cfg_and_block_env(
            &mut cfg_env,
            &mut block_env,
            &chain_spec,
            &header,
            total_difficulty,
        );

        // Assert that the chain ID in the `cfg_env` is correctly set to the chain ID of the
        // ChainSpec
        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }

    #[test]
    fn test_evm_configure() {
        // Create a default `OptimismEvmConfig`
        let evm_config = OptimismEvmConfig::default();

        // Initialize an empty database wrapped in CacheDB
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create an EVM instance using the configuration and the database
        let evm = evm_config.evm(db);

        // Check that the EVM environment is initialized with default values
        assert_eq!(evm.context.evm.inner.env, Box::default());

        // Latest spec ID and no warm preloaded addresses
        assert_eq!(
            evm.context.evm.inner.journaled_state,
            JournaledState::new(SpecId::LATEST, HashSet::default())
        );

        // Ensure that the accounts database is empty
        assert!(evm.context.evm.inner.db.accounts.is_empty());

        // Ensure that the block hashes database is empty
        assert!(evm.context.evm.inner.db.block_hashes.is_empty());

        // Verify that there are two default contracts in the contracts database
        assert_eq!(evm.context.evm.inner.db.contracts.len(), 2);
        assert!(evm.context.evm.inner.db.contracts.contains_key(&KECCAK_EMPTY));
        assert!(evm.context.evm.inner.db.contracts.contains_key(&B256::ZERO));

        // Ensure that the logs database is empty
        assert!(evm.context.evm.inner.db.logs.is_empty());

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);
    }

    #[test]
    fn test_evm_with_env_default_spec() {
        let evm_config = OptimismEvmConfig::default();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let env_with_handler = EnvWithHandlerCfg::default();

        let evm = evm_config.evm_with_env(db, env_with_handler.clone());

        // Check that the EVM environment
        assert_eq!(evm.context.evm.env, env_with_handler.env);

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });
    }

    #[test]
    fn test_evm_with_env_custom_cfg() {
        let evm_config = OptimismEvmConfig::default();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create a custom configuration environment with a chain ID of 111
        let cfg = CfgEnv::default().with_chain_id(111);

        let env_with_handler = EnvWithHandlerCfg {
            env: Box::new(Env {
                cfg: cfg.clone(),
                block: BlockEnv::default(),
                tx: TxEnv::default(),
            }),
            handler_cfg: Default::default(),
        };

        let evm = evm_config.evm_with_env(db, env_with_handler);

        // Check that the EVM environment is initialized with the custom environment
        assert_eq!(evm.context.evm.inner.env.cfg, cfg);

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });
    }

    #[test]
    fn test_evm_with_env_custom_block_and_tx() {
        let evm_config = OptimismEvmConfig::default();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create customs block and tx env
        let block = BlockEnv {
            basefee: U256::from(1000),
            gas_limit: U256::from(10_000_000),
            number: U256::from(42),
            ..Default::default()
        };
        let tx = TxEnv { gas_limit: 5_000_000, gas_price: U256::from(50), ..Default::default() };

        let env_with_handler = EnvWithHandlerCfg {
            env: Box::new(Env { cfg: CfgEnv::default(), block, tx }),
            handler_cfg: Default::default(),
        };

        let evm = evm_config.evm_with_env(db, env_with_handler.clone());

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.context.evm.env.block, env_with_handler.env.block);
        assert_eq!(evm.context.evm.env.tx, env_with_handler.env.tx);

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });
    }

    #[test]
    fn test_evm_with_spec_id() {
        let evm_config = OptimismEvmConfig::default();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let handler_cfg = HandlerCfg { spec_id: SpecId::ECOTONE, ..Default::default() };

        let env_with_handler = EnvWithHandlerCfg { env: Box::new(Env::default()), handler_cfg };

        let evm = evm_config.evm_with_env(db, env_with_handler);

        // Check that the spec ID is setup properly
        assert_eq!(evm.handler.spec_id(), SpecId::ECOTONE);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::ECOTONE, is_optimism: true });
    }

    #[test]
    fn test_evm_with_inspector() {
        let evm_config = OptimismEvmConfig::default();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // No operation inspector
        let noop = NoOpInspector;

        let evm = evm_config.evm_with_inspector(db, noop);

        // Check that the inspector is set correctly
        assert_eq!(evm.context.external, noop);

        // Check that the EVM environment is initialized with default values
        assert_eq!(evm.context.evm.inner.env, Box::default());

        // Latest spec ID and no warm preloaded addresses
        assert_eq!(
            evm.context.evm.inner.journaled_state,
            JournaledState::new(SpecId::LATEST, HashSet::default())
        );

        // Ensure that the accounts database is empty
        assert!(evm.context.evm.inner.db.accounts.is_empty());

        // Ensure that the block hashes database is empty
        assert!(evm.context.evm.inner.db.block_hashes.is_empty());

        // Verify that there are two default contracts in the contracts database
        assert_eq!(evm.context.evm.inner.db.contracts.len(), 2);
        assert!(evm.context.evm.inner.db.contracts.contains_key(&KECCAK_EMPTY));
        assert!(evm.context.evm.inner.db.contracts.contains_key(&B256::ZERO));

        // Ensure that the logs database is empty
        assert!(evm.context.evm.inner.db.logs.is_empty());

        // Default spec ID
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });
    }

    #[test]
    fn test_evm_with_env_and_default_inspector() {
        let evm_config = OptimismEvmConfig::default();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let env_with_handler = EnvWithHandlerCfg::default();

        let evm =
            evm_config.evm_with_env_and_inspector(db, env_with_handler.clone(), NoOpInspector);

        // Check that the EVM environment is set to default values
        assert_eq!(evm.context.evm.env, env_with_handler.env);
        assert_eq!(evm.context.external, NoOpInspector);
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_cfg() {
        let evm_config = OptimismEvmConfig::default();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let cfg = CfgEnv::default().with_chain_id(111);
        let block = BlockEnv::default();
        let tx = TxEnv::default();
        let env_with_handler = EnvWithHandlerCfg {
            env: Box::new(Env { cfg: cfg.clone(), block, tx }),
            handler_cfg: Default::default(),
        };

        let evm = evm_config.evm_with_env_and_inspector(db, env_with_handler, NoOpInspector);

        // Check that the EVM environment is set with custom configuration
        assert_eq!(evm.context.evm.env.cfg, cfg);
        assert_eq!(evm.context.external, NoOpInspector);
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_block_tx() {
        let evm_config = OptimismEvmConfig::default();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create custom block and tx environment
        let block = BlockEnv {
            basefee: U256::from(1000),
            gas_limit: U256::from(10_000_000),
            number: U256::from(42),
            ..Default::default()
        };
        let tx = TxEnv { gas_limit: 5_000_000, gas_price: U256::from(50), ..Default::default() };
        let env_with_handler = EnvWithHandlerCfg {
            env: Box::new(Env { cfg: CfgEnv::default(), block, tx }),
            handler_cfg: Default::default(),
        };

        let evm =
            evm_config.evm_with_env_and_inspector(db, env_with_handler.clone(), NoOpInspector);

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.context.evm.env.block, env_with_handler.env.block);
        assert_eq!(evm.context.evm.env.tx, env_with_handler.env.tx);
        assert_eq!(evm.context.external, NoOpInspector);
        assert_eq!(evm.handler.spec_id(), SpecId::LATEST);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true });
    }

    #[test]
    fn test_evm_with_env_inspector_and_spec_id() {
        let evm_config = OptimismEvmConfig::default();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let handler_cfg = HandlerCfg { spec_id: SpecId::ECOTONE, ..Default::default() };
        let env_with_handler = EnvWithHandlerCfg { env: Box::new(Env::default()), handler_cfg };

        let evm =
            evm_config.evm_with_env_and_inspector(db, env_with_handler.clone(), NoOpInspector);

        // Check that the spec ID is set properly
        assert_eq!(evm.handler.spec_id(), SpecId::ECOTONE);
        assert_eq!(evm.context.evm.env, env_with_handler.env);
        assert_eq!(evm.context.external, NoOpInspector);

        // Check that the spec ID is setup properly
        assert_eq!(evm.handler.spec_id(), SpecId::ECOTONE);

        // Optimism in handler
        assert_eq!(evm.handler.cfg, HandlerCfg { spec_id: SpecId::ECOTONE, is_optimism: true });
    }
}
