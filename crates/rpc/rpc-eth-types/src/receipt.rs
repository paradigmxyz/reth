//! RPC receipt response builder, extends a layer one receipt with layer two data.

use alloy_consensus::{ReceiptEnvelope, Transaction};
use alloy_primitives::{Address, TxKind};
use alloy_rpc_types::{Log, ReceiptWithBloom, TransactionReceipt};
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned, TxType};
use revm_primitives::calc_blob_gasprice;

use super::{EthApiError, EthResult};

/// Builds an [`TransactionReceipt`] obtaining the inner receipt envelope from the given closure.
pub fn build_receipt<T>(
    transaction: &TransactionSigned,
    meta: TransactionMeta,
    receipt: &Receipt,
    all_receipts: &[Receipt],
    build_envelope: impl FnOnce(ReceiptWithBloom<Log>) -> T,
) -> EthResult<TransactionReceipt<T>> {
    // Note: we assume this transaction is valid, because it's mined (or part of pending block)
    // and we don't need to check for pre EIP-2
    let from =
        transaction.recover_signer_unchecked().ok_or(EthApiError::InvalidTransactionSignature)?;

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
    let blob_gas_price = blob_gas_used.and_then(|_| meta.excess_blob_gas.map(calc_blob_gasprice));
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

    Ok(TransactionReceipt {
        inner: build_envelope(ReceiptWithBloom { receipt: rpc_receipt, logs_bloom }),
        transaction_hash: meta.tx_hash,
        transaction_index: Some(meta.index),
        block_hash: Some(meta.block_hash),
        block_number: Some(meta.block_number),
        from,
        to,
        gas_used: gas_used as u128,
        contract_address,
        effective_gas_price: transaction.effective_gas_price(meta.base_fee),
        // EIP-4844 fields
        blob_gas_price,
        blob_gas_used: blob_gas_used.map(u128::from),
        authorization_list: transaction.authorization_list().map(|l| l.to_vec()),
    })
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
        transaction: &TransactionSigned,
        meta: TransactionMeta,
        receipt: &Receipt,
        all_receipts: &[Receipt],
    ) -> EthResult<Self> {
        let base = build_receipt(transaction, meta, receipt, all_receipts, |receipt_with_bloom| {
            match receipt.tx_type {
                TxType::Legacy => ReceiptEnvelope::Legacy(receipt_with_bloom),
                TxType::Eip2930 => ReceiptEnvelope::Eip2930(receipt_with_bloom),
                TxType::Eip1559 => ReceiptEnvelope::Eip1559(receipt_with_bloom),
                TxType::Eip4844 => ReceiptEnvelope::Eip4844(receipt_with_bloom),
                TxType::Eip7702 => ReceiptEnvelope::Eip7702(receipt_with_bloom),
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        })?;

        Ok(Self { base })
    }

    /// Builds a receipt response from the base response body, and any set additional fields.
    pub fn build(self) -> TransactionReceipt {
        self.base
    }
}
