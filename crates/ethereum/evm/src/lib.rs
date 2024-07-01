//! EVM config for vanilla ethereum.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

use reth_chainspec::{ChainSpec, Head};
use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{Address, Header, Transaction, TransactionSigned, TxKind, U256};
use reth_revm::{Database, EvmBuilder};
use revm_primitives::{AnalysisKind, CfgEnvWithHandlerCfg, TxEnv};

mod config;
pub use config::{revm_spec, revm_spec_by_timestamp_after_merge};

pub mod execute;

/// Ethereum DAO hardfork state change data.
pub mod dao_fork;

/// [EIP-6110](https://eips.ethereum.org/EIPS/eip-6110) handling.
pub mod eip6110;

/// Ethereum-related EVM configuration.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct EthEvmConfig;

impl ConfigureEvmEnv for EthEvmConfig {
    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        let spec_id = config::revm_spec(
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
    }

    fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
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
            Transaction::Eip4844(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = TxKind::Call(tx.to);
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
                tx_env.blob_hashes.clone_from(&tx.blob_versioned_hashes);
                tx_env.max_fee_per_blob_gas = Some(U256::from(tx.max_fee_per_blob_gas));
            }
        }
    }
}

impl ConfigureEvm for EthEvmConfig {
    type DefaultExternalContext<'a> = ();

    fn evm<'a, DB: Database + 'a>(
        &self,
        db: DB,
    ) -> reth_revm::Evm<'a, Self::DefaultExternalContext<'a>, DB> {
        EvmBuilder::default().with_db(db).build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ChainSpec;
    use reth_primitives::{
        revm_primitives::{BlockEnv, CfgEnv, SpecId},
        Header, U256,
    };
    use revm_primitives::CfgEnvWithHandlerCfg;

    #[test]
    #[ignore]
    fn test_fill_cfg_and_block_env() {
        let mut cfg_env = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);
        let mut block_env = BlockEnv::default();
        let header = Header::default();
        let chain_spec = ChainSpec::default();
        let total_difficulty = U256::ZERO;

        EthEvmConfig::fill_cfg_and_block_env(
            &mut cfg_env,
            &mut block_env,
            &chain_spec,
            &header,
            total_difficulty,
        );

        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }
}
