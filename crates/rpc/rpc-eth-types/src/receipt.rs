//! RPC receipt response builder, extends a layer one receipt with layer two data.

use alloy_consensus::{
    transaction::{Recovered, SignerRecoverable, TransactionMeta},
    ReceiptEnvelope, Transaction, TxReceipt,
};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, TxKind};
use alloy_rpc_types_eth::{Log, ReceiptWithBloom, TransactionReceipt};
use reth_ethereum_primitives::{Receipt, TransactionSigned};

/// Builds an [`TransactionReceipt`] obtaining the inner receipt envelope from the given closure.
pub fn build_receipt<R, T, E>(
    transaction: Recovered<&T>,
    meta: TransactionMeta,
    receipt: &R,
    all_receipts: &[R],
    blob_params: Option<BlobParams>,
    build_envelope: impl FnOnce(ReceiptWithBloom<alloy_consensus::Receipt<Log>>) -> E,
) -> TransactionReceipt<E>
where
    R: TxReceipt<Log = alloy_primitives::Log>,
    T: Transaction + SignerRecoverable,
{
    let from = transaction.signer();

    // get the previous transaction cumulative gas used
    let gas_used = if meta.index == 0 {
        receipt.cumulative_gas_used()
    } else {
        let prev_tx_idx = (meta.index - 1) as usize;
        all_receipts
            .get(prev_tx_idx)
            .map(|prev_receipt| receipt.cumulative_gas_used() - prev_receipt.cumulative_gas_used())
            .unwrap_or_default()
    };

    let blob_gas_used = transaction.blob_gas_used();
    // Blob gas price should only be present if the transaction is a blob transaction
    let blob_gas_price =
        blob_gas_used.and_then(|_| Some(blob_params?.calc_blob_fee(meta.excess_blob_gas?)));

    let logs_bloom = receipt.bloom();

    // get number of logs in the block
    let mut num_logs = 0;
    for prev_receipt in all_receipts.iter().take(meta.index as usize) {
        num_logs += prev_receipt.logs().len();
    }

    let logs: Vec<Log> = receipt
        .logs()
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

    let rpc_receipt = alloy_rpc_types_eth::Receipt {
        status: receipt.status_or_post_state(),
        cumulative_gas_used: receipt.cumulative_gas_used(),
        logs,
    };

    let (contract_address, to) = match transaction.kind() {
        TxKind::Create => (Some(from.create(transaction.nonce())), None),
        TxKind::Call(addr) => (None, Some(Address(*addr))),
    };

    TransactionReceipt {
        inner: build_envelope(ReceiptWithBloom { receipt: rpc_receipt, logs_bloom }),
        transaction_hash: meta.tx_hash,
        transaction_index: Some(meta.index),
        block_hash: Some(meta.block_hash),
        block_number: Some(meta.block_number),
        from,
        to,
        gas_used,
        contract_address,
        effective_gas_price: transaction.effective_gas_price(meta.base_fee),
        // EIP-4844 fields
        blob_gas_price,
        blob_gas_used,
    }
}

/// Receipt response builder.
#[derive(Debug)]
pub struct EthReceiptBuilder {
    /// The base response body, contains L1 fields.
    pub base: TransactionReceipt,
}

impl EthReceiptBuilder {
    /// Returns a new builder with the base response body (L1 fields) set.
    ///
    /// Note: This requires _all_ block receipts because we need to calculate the gas used by the
    /// transaction.
    pub fn new(
        transaction: Recovered<&TransactionSigned>,
        meta: TransactionMeta,
        receipt: &Receipt,
        all_receipts: &[Receipt],
        blob_params: Option<BlobParams>,
    ) -> Self {
        let base = build_receipt(
            transaction,
            meta,
            receipt,
            all_receipts,
            blob_params,
            |receipt_with_bloom| ReceiptEnvelope::from_typed(receipt.tx_type, receipt_with_bloom),
        );

        Self { base }
    }

    /// Builds a receipt response from the base response body, and any set additional fields.
    pub fn build(self) -> TransactionReceipt {
        self.base
    }
}
