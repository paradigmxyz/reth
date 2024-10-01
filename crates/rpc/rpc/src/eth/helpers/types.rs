//! L1 `eth` API types.

use alloy_network::{AnyNetwork, Network};
use alloy_primitives::{Address, TxKind};
use alloy_rpc_types::{Transaction, TransactionInfo};
use alloy_serde::WithOtherFields;
use reth_primitives::TransactionSignedEcRecovered;
use reth_rpc_types_compat::{
    transaction::{from_primitive_signature, GasPrice},
    TransactionCompat,
};

/// Builds RPC transaction response for l1.
#[derive(Debug, Clone, Copy)]
pub struct EthTxBuilder;

impl TransactionCompat for EthTxBuilder
where
    Self: Send + Sync,
{
    type Transaction = <AnyNetwork as Network>::TransactionResponse;

    fn fill(tx: TransactionSignedEcRecovered, tx_info: TransactionInfo) -> Self::Transaction {
        let signer = tx.signer();
        let signed_tx = tx.into_signed();

        let to: Option<Address> = match signed_tx.kind() {
            TxKind::Create => None,
            TxKind::Call(to) => Some(Address(*to)),
        };

        let TransactionInfo {
            base_fee, block_hash, block_number, index: transaction_index, ..
        } = tx_info;

        let GasPrice { gas_price, max_fee_per_gas } =
            Self::gas_price(&signed_tx, base_fee.map(|fee| fee as u64));

        let chain_id = signed_tx.chain_id();
        let blob_versioned_hashes = signed_tx.blob_versioned_hashes();
        let access_list = signed_tx.access_list().cloned();
        let authorization_list = signed_tx.authorization_list().map(|l| l.to_vec());

        let signature = from_primitive_signature(
            *signed_tx.signature(),
            signed_tx.tx_type(),
            signed_tx.chain_id(),
        );

        WithOtherFields {
            inner: Transaction {
                hash: signed_tx.hash(),
                nonce: signed_tx.nonce(),
                from: signer,
                to,
                value: signed_tx.value(),
                gas_price,
                max_fee_per_gas,
                max_priority_fee_per_gas: signed_tx.max_priority_fee_per_gas(),
                signature: Some(signature),
                gas: signed_tx.gas_limit(),
                input: signed_tx.input().clone(),
                chain_id,
                access_list,
                transaction_type: Some(signed_tx.tx_type() as u8),
                // These fields are set to None because they are not stored as part of the
                // transaction
                block_hash,
                block_number,
                transaction_index,
                // EIP-4844 fields
                max_fee_per_blob_gas: signed_tx.max_fee_per_blob_gas(),
                blob_versioned_hashes,
                authorization_list,
            },
            ..Default::default()
        }
    }

    fn otterscan_api_truncate_input(tx: &mut Self::Transaction) {
        tx.inner.input = tx.inner.input.slice(..4);
    }

    fn tx_type(tx: &Self::Transaction) -> u8 {
        tx.inner.transaction_type.unwrap_or(0)
    }
}
