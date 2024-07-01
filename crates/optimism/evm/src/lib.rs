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
    Address, Head, Header, Transaction, TransactionSigned, TxKind, U256,
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
use revm_primitives::OptimismFields;

/// Optimism-related EVM configuration.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct OptimismEvmConfig;

impl ConfigureEvmEnv for OptimismEvmConfig {
    fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
        let mut envelope = Vec::with_capacity(transaction.length_without_header());
        transaction.encode_enveloped(&mut envelope);

        tx_env.caller = sender;
        match transaction.as_ref() {
            Transaction::Legacy(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.gas_price);
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = tx.chain_id;
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clear();
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
            }
            Transaction::Eip2930(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.gas_price);
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list = tx
                    .access_list
                    .iter()
                    .map(|l| {
                        (
                            l.address,
                            l.storage_keys.iter().map(|k| U256::from_be_bytes(k.0)).collect(),
                        )
                    })
                    .collect();
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
            }
            Transaction::Eip1559(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list = tx
                    .access_list
                    .iter()
                    .map(|l| {
                        (
                            l.address,
                            l.storage_keys.iter().map(|k| U256::from_be_bytes(k.0)).collect(),
                        )
                    })
                    .collect();
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
            }
            Transaction::Eip4844(_) => {
                panic!("No eip4844 tx in optimism")
            }
            Transaction::Deposit(tx) => {
                tx_env.access_list.clear();
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::ZERO;
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = None;
                tx_env.nonce = None;
                tx_env.optimism = OptimismFields {
                    source_hash: Some(tx.source_hash),
                    mint: tx.mint,
                    is_system_transaction: Some(tx.is_system_transaction),
                    enveloped_tx: Some(envelope.into()),
                };
                return;
            }
        }

        if !transaction.is_deposit() {
            tx_env.optimism = OptimismFields {
                source_hash: None,
                mint: None,
                is_system_transaction: Some(false),
                enveloped_tx: Some(envelope.into()),
            }
        }
    }

    fn fill_cfg_env(
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

    fn evm<'a, DB: Database + 'a>(&self, db: DB) -> Evm<'a, Self::DefaultExternalContext<'a>, DB> {
        EvmBuilder::default().with_db(db).optimism().build()
    }

    fn evm_with_inspector<'a, DB, I>(&self, db: DB, inspector: I) -> Evm<'a, I, DB>
    where
        DB: Database + 'a,
        I: GetInspector<DB>,
    {
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            .optimism()
            .append_handler_register(inspector_handle_register)
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::revm_primitives::{BlockEnv, CfgEnv};
    use revm_primitives::SpecId;

    #[test]
    #[ignore]
    fn test_fill_cfg_and_block_env() {
        let mut cfg_env = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);
        let mut block_env = BlockEnv::default();
        let header = Header::default();
        let chain_spec = ChainSpec::default();
        let total_difficulty = U256::ZERO;

        OptimismEvmConfig::fill_cfg_and_block_env(
            &mut cfg_env,
            &mut block_env,
            &chain_spec,
            &header,
            total_difficulty,
        );

        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }
}
