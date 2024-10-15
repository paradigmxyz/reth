//! RPC receipt response builder, extends a layer one receipt with layer two data.

use alloy_primitives::{Address, TxKind};
use alloy_rpc_types::{
    AnyReceiptEnvelope, AnyTransactionReceipt, Log, ReceiptWithBloom, TransactionReceipt,
};
use alloy_serde::{OtherFields, WithOtherFields};
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned};
use revm_primitives::calc_blob_gasprice;

use super::{EthApiError, EthResult};

/// Receipt response builder.
#[derive(Debug)]
pub struct ReceiptBuilder {
    /// The base response body, contains L1 fields.
    pub base: TransactionReceipt<AnyReceiptEnvelope<Log>>,
    /// Additional L2 fields.
    pub other: OtherFields,
}

impl ReceiptBuilder {
    /// Returns a new builder with the base response body (L1 fields) set.
    ///
    /// Note: This requires _all_ block receipts because we need to calculate the gas used by the
    /// transaction.
    pub fn new(
        transaction: &TransactionSigned,
        meta: TransactionMeta,
        receipt: &Receipt,
        all_receipts: &[Receipt],
    ) -> EthResult<Self> {
        // Note: we assume this transaction is valid, because it's mined (or part of pending block)
        // and we don't need to check for pre EIP-2
        let from = transaction
            .recover_signer_unchecked()
            .ok_or(EthApiError::InvalidTransactionSignature)?;

        // get the previous transaction cumulative gas used
        let gas_used = if meta.index == 0 {
            receipt.cumulative_gas_used
        } else {
            let prev_tx_idx = (meta.index - 1) as usize;
            all_receipts
                .get(prev_tx_idx)
                .map(|prev_receipt| receipt.cumulative_gas_used - prev_receipt.cumulative_gas_used)
                .unwrap_or_default()
        };

        let blob_gas_used = transaction.transaction.blob_gas_used();
        // Blob gas price should only be present if the transaction is a blob transaction
        let blob_gas_price =
            blob_gas_used.and_then(|_| meta.excess_blob_gas.map(calc_blob_gasprice));
        let logs_bloom = receipt.bloom_slow();

        // get number of logs in the block
        let mut num_logs = 0;
        for prev_receipt in all_receipts.iter().take(meta.index as usize) {
            num_logs += prev_receipt.logs.len();
        }

        let logs: Vec<Log> = receipt
            .logs
            .iter()
            .enumerate()
            .map(|(tx_log_idx, log)| Log {
                inner: log.clone(),
                block_hash: Some(meta.block_hash),
                block_number: Some(meta.block_number),
                block_timestamp: Some(meta.timestamp),
                transaction_hash: Some(meta.tx_hash),
                transaction_index: Some(meta.index),
                log_index: Some((num_logs + tx_log_idx) as u64),
                removed: false,
            })
            .collect();

        let rpc_receipt = alloy_rpc_types::Receipt {
            status: receipt.success.into(),
            cumulative_gas_used: receipt.cumulative_gas_used as u128,
            logs,
        };

        let (contract_address, to) = match transaction.transaction.kind() {
            TxKind::Create => (Some(from.create(transaction.transaction.nonce())), None),
            TxKind::Call(addr) => (None, Some(Address(*addr))),
        };

        #[allow(clippy::needless_update)]
        let base = TransactionReceipt {
            inner: AnyReceiptEnvelope {
                inner: ReceiptWithBloom { receipt: rpc_receipt, logs_bloom },
                r#type: transaction.transaction.tx_type().into(),
            },
            transaction_hash: meta.tx_hash,
            transaction_index: Some(meta.index),
            block_hash: Some(meta.block_hash),
            block_number: Some(meta.block_number),
            from,
            to,
            gas_used: gas_used as u128,
            contract_address,
            effective_gas_price: transaction.effective_gas_price(meta.base_fee),
            // TODO pre-byzantium receipts have a post-transaction state root
            state_root: None,
            // EIP-4844 fields
            blob_gas_price,
            blob_gas_used: blob_gas_used.map(u128::from),
            authorization_list: transaction.authorization_list().map(|l| l.to_vec()),
        };

        Ok(Self { base, other: Default::default() })
    }

    /// Adds fields to response body.
    pub fn add_other_fields(mut self, mut fields: OtherFields) -> Self {
        self.other.append(&mut fields);
        self
    }

    /// Builds a receipt response from the base response body, and any set additional fields.
    pub fn build(self) -> AnyTransactionReceipt {
        let Self { base, other } = self;
        WithOtherFields { inner: base, other }
    }
}
