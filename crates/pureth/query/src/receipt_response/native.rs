use super::{verify_whole_receipt_proof, WholeReceiptProof};
use crate::{
    selection::{add, encoded_size, remaining_materialization, source_budget},
    SelectionError, SelectionLimits,
};
use alloy_consensus::{Transaction, TxReceipt};
use alloy_primitives::{Address, TxKind, B256, U64};
use alloy_rpc_types_eth::TransactionReceipt;
use reth_ethereum_primitives::Block;
use reth_primitives_traits::RecoveredBlock;
use reth_pureth_receipt::ProviderSnapshot;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvenReceiptFields {
    block_hash: B256,
    block_number: U64,
    transaction_hash: B256,
    transaction_index: U64,
    from: Address,
    gas_used: U64,
    status: U64,
    contract_address: Option<Address>,
    authorities: Option<Vec<Address>>,
    logs: Vec<alloy_rpc_types_eth::Log>,
}

#[derive(Serialize)]
struct ProvenReceiptResponse<'a> {
    #[serde(flatten)]
    receipt: &'a serde_json::Value,
    #[serde(rename = "sszProof")]
    proof: &'a WholeReceiptProof,
}

#[derive(Serialize)]
struct Eip6466ReceiptFields<'a> {
    #[serde(rename = "type")]
    transaction_type: U64,
    status: U64,
    logs: Vec<alloy_rpc_types_eth::Log>,
    authorities: Option<&'a [Address]>,
}

struct ConversionBudget {
    source: usize,
    native_bytes: usize,
    value_bytes: usize,
}

pub fn convert_eip6466_receipt(
    receipt: TransactionReceipt,
    block: &RecoveredBlock<Block>,
    snapshot: &ProviderSnapshot,
    index: u64,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<serde_json::Value, SelectionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    let source = source_budget(snapshot.receipt_snapshot(), limits, cancelled)?;
    let mut budget = ConversionBudget { source, native_bytes: 0, value_bytes: 0 };
    convert_with_source(receipt, block, snapshot, index, limits, cancelled, &mut budget)
}

pub fn convert_eip6466_block_receipts(
    receipts: Vec<TransactionReceipt>,
    block: &RecoveredBlock<Block>,
    snapshot: &ProviderSnapshot,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<Vec<serde_json::Value>, SelectionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    if snapshot.object() != reth_pureth_receipt::ObjectKind::Receipts ||
        block.hash() != snapshot.block_hash() ||
        receipts.len() != snapshot.receipt_snapshot().receipts().len() ||
        receipts.len() != block.body().transactions.len()
    {
        return Err(SelectionError::InvalidContext);
    }
    let native_bytes = encoded_size(&receipts, limits.response_bytes, "response bytes")?;
    let source = source_budget(snapshot.receipt_snapshot(), limits, cancelled)?;
    remaining_materialization(source, native_bytes, 0, limits.source_bytes)?;
    let mut budget = ConversionBudget { source, native_bytes, value_bytes: 2 };
    let mut values = Vec::new();
    for (index, receipt) in receipts.into_iter().enumerate() {
        if index != 0 {
            add(&mut budget.value_bytes, 1, limits.response_bytes, "response bytes")?;
        }
        values.push(convert_with_source(
            receipt,
            block,
            snapshot,
            index as u64,
            limits,
            cancelled,
            &mut budget,
        )?);
    }
    let value_bytes = encoded_size(&values, limits.response_bytes, "response bytes")?;
    remaining_materialization(source, native_bytes.max(value_bytes), 0, limits.source_bytes)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    Ok(values)
}

fn convert_with_source(
    receipt: TransactionReceipt,
    block: &RecoveredBlock<Block>,
    snapshot: &ProviderSnapshot,
    index: u64,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
    budget: &mut ConversionBudget,
) -> Result<serde_json::Value, SelectionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    let native_bytes = encoded_size(&receipt, limits.response_bytes, "response bytes")?;
    budget.native_bytes = budget.native_bytes.max(native_bytes);
    remaining_materialization(
        budget.source,
        budget.native_bytes.max(budget.value_bytes),
        0,
        limits.source_bytes,
    )?;
    let position = usize::try_from(index).map_err(|_| SelectionError::OutOfBounds)?;
    let transaction = block.body().transactions.get(position).ok_or(SelectionError::OutOfBounds)?;
    let retained = snapshot.receipt_snapshot().receipts();
    let expected = retained.get(position).ok_or(SelectionError::OutOfBounds)?;
    if snapshot.object() != reth_pureth_receipt::ObjectKind::Receipts ||
        snapshot.block_hash() != block.hash() ||
        retained.len() != block.body().transactions.len() ||
        receipt.block_hash != Some(block.hash()) ||
        receipt.block_number != Some(block.header().number) ||
        receipt.transaction_index != Some(index) ||
        receipt.transaction_hash != *transaction.hash() ||
        receipt.from != expected.from_() ||
        receipt.gas_used != expected.gas_used() ||
        receipt.inner.status_or_post_state().as_eip658() != Some(expected.status()) ||
        receipt.transaction_type() != transaction.tx_type() ||
        receipt.to != transaction.kind().to().copied()
    {
        return Err(SelectionError::InvalidContext);
    }
    let candidate = match transaction.kind() {
        TxKind::Create => Some(expected.from_().create(transaction.nonce())),
        TxKind::Call(_) => None,
    };
    if receipt.contract_address != candidate &&
        receipt.contract_address != expected.contract_address()
    {
        return Err(SelectionError::InvalidContext);
    }
    let logs = receipt.inner.logs();
    if logs.len() != expected.logs().len() {
        return Err(SelectionError::InvalidValue);
    }
    for (log, expected) in logs.iter().zip(expected.logs()) {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        if log.inner.address != expected.address() ||
            log.inner.data.topics() != expected.topics() ||
            log.inner.data.data != *expected.data()
        {
            return Err(SelectionError::InvalidValue);
        }
        if log.block_hash != receipt.block_hash ||
            log.block_number != receipt.block_number ||
            log.transaction_hash != Some(receipt.transaction_hash) ||
            log.transaction_index != Some(index) ||
            log.removed
        {
            return Err(SelectionError::InvalidContext);
        }
    }
    let mut receipt = receipt;
    receipt.contract_address = expected.contract_address();
    let mut receipt = receipt.map_inner(|inner| Eip6466ReceiptFields {
        transaction_type: U64::from(inner.tx_type() as u8),
        status: U64::from(u64::from(expected.status())),
        logs: inner.into_logs(),
        authorities: expected.authorities(),
    });
    for (position, log) in receipt.inner.logs.iter_mut().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        log.log_index = Some(position as u64);
    }
    let available = limits
        .response_bytes
        .checked_sub(budget.value_bytes)
        .ok_or(SelectionError::LimitExceeded("response bytes"))?;
    let value_bytes = encoded_size(&receipt, available, "response bytes")?;
    add(&mut budget.value_bytes, value_bytes, limits.response_bytes, "response bytes")?;
    remaining_materialization(
        budget.source,
        budget.native_bytes.max(budget.value_bytes),
        0,
        limits.source_bytes,
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    let value = serde_json::to_value(&receipt).map_err(|_| SelectionError::InvalidValue)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    Ok(value)
}

pub fn verify_eip6466_receipt_response(
    receipt: &serde_json::Value,
    proof: &WholeReceiptProof,
    block_hash: B256,
    index: u64,
    transaction_hash: B256,
    expected_root: B256,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    let receipt_bytes = encoded_size(receipt, limits.response_bytes, "response bytes")?;
    encoded_size(
        &ProvenReceiptResponse { receipt, proof },
        limits.response_bytes,
        "response bytes",
    )?;
    let object = receipt.as_object().ok_or(SelectionError::InvalidValue)?;
    if object.contains_key("logsBloom") ||
        object.contains_key("cumulativeGasUsed") ||
        object.contains_key("root") ||
        object.contains_key("sszProof") ||
        !object.contains_key("authorities") ||
        !object.contains_key("contractAddress")
    {
        return Err(SelectionError::InvalidContext);
    }
    verify_whole_receipt_proof(block_hash, index, proof, expected_root, limits)?;
    let bytes = proof
        .selection
        .results
        .first()
        .and_then(|result| result.values_ssz.first())
        .ok_or(SelectionError::InvalidProof)?;
    let value_bytes = receipt_bytes
        .checked_add(bytes.len())
        .ok_or(SelectionError::LimitExceeded("source bytes"))?;
    remaining_materialization(0, value_bytes, 0, limits.source_bytes)?;
    let fields =
        ProvenReceiptFields::deserialize(receipt).map_err(|_| SelectionError::InvalidValue)?;
    let expected = reth_pureth_receipt::Receipt::from_ssz_bytes(bytes)
        .map_err(|_| SelectionError::InvalidValue)?;
    if fields.block_hash != block_hash ||
        fields.transaction_index != U64::from(index) ||
        fields.transaction_hash != transaction_hash
    {
        return Err(SelectionError::InvalidContext);
    }
    if fields.from != expected.from_() ||
        fields.gas_used != U64::from(expected.gas_used()) ||
        fields.status != U64::from(u64::from(expected.status())) ||
        fields.contract_address != expected.contract_address() ||
        fields.authorities.as_deref() != expected.authorities() ||
        fields.logs.len() != expected.logs().len()
    {
        return Err(SelectionError::InvalidValue);
    }
    for (position, (log, expected)) in fields.logs.iter().zip(expected.logs()).enumerate() {
        if log.inner.address != expected.address() ||
            log.inner.data.topics() != expected.topics() ||
            log.inner.data.data != *expected.data()
        {
            return Err(SelectionError::InvalidValue);
        }
        if log.block_hash != Some(block_hash) ||
            log.block_number != Some(fields.block_number.to::<u64>()) ||
            log.transaction_hash != Some(fields.transaction_hash) ||
            log.transaction_index != Some(index) ||
            log.log_index != Some(position as u64) ||
            log.removed
        {
            return Err(SelectionError::InvalidContext);
        }
    }
    Ok(())
}
