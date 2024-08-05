use crate::{Address, Transaction, TransactionSigned, TxKind, U256};
use revm_primitives::{AuthorizationList, TxEnv};


#[cfg(all(not(feature = "std"), feature = "optimism"))]
use alloc::vec::Vec;

/// Implements behaviour to fill a [`TxEnv`] from another transaction.
pub trait FillTxEnv {
    /// Fills [`TxEnv`] with an [`Address`] and transaction.
    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address);
}

impl FillTxEnv for TransactionSigned {
    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address) {
        #[cfg(feature = "optimism")]
        let envelope = {
            let mut envelope = Vec::with_capacity(self.length_without_header());
            self.encode_enveloped(&mut envelope);
            envelope
        };

        tx_env.caller = sender;
        match self.as_ref() {
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
                tx_env.access_list.clone_from(&tx.access_list.0);
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
                tx_env.access_list.clone_from(&tx.access_list.0);
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
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clone_from(&tx.blob_versioned_hashes);
                tx_env.max_fee_per_blob_gas = Some(U256::from(tx.max_fee_per_blob_gas));
            }
            Transaction::Eip7702(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list =
                    Some(AuthorizationList::Signed(tx.authorization_list.clone()));
            }
            #[cfg(feature = "optimism")]
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
                tx_env.optimism = revm_primitives::OptimismFields {
                    source_hash: Some(tx.source_hash),
                    mint: tx.mint,
                    is_system_transaction: Some(tx.is_system_transaction),
                    enveloped_tx: Some(envelope.into()),
                };
                return;
            }
        }

        #[cfg(feature = "optimism")]
        if !self.is_deposit() {
            tx_env.optimism = revm_primitives::OptimismFields {
                source_hash: None,
                mint: None,
                is_system_transaction: Some(false),
                enveloped_tx: Some(envelope.into()),
            }
        }
    }
}
