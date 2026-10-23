use crate::{
    components::{select_component_with_usage, verify_component_response_with_usage},
    selection::{
        add, encoded_size, multiproof, receipt_verification_work, select_receipt_with_usage,
    },
    verify_receipt_selection, ComponentObject, ComponentSnapshot, ReceiptSelection, SelectionError,
    SelectionLimits, SelectionOperation, SelectionRequest, SelectionResponse,
};
use alloy_primitives::B256;
use reth_pureth_block::ExecutionPayloadSnapshot;
use reth_pureth_transaction::decode_transaction;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_arch = "wasm32"))]
use crate::{query_service::block_status, QueryError};
#[cfg(not(target_arch = "wasm32"))]
use reth_pureth_block::SourcePayloadSnapshot;
#[cfg(not(target_arch = "wasm32"))]
use reth_pureth_receipt::{LookupError, RootContext};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionReceiptRequest {
    pub block_hash: B256,
    pub transaction_index: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionReceiptResponse {
    pub block_hash: B256,
    pub transaction_index: u64,
    pub root_context: String,
    pub block_status: String,
    pub transaction_hash: B256,
    pub transaction: SelectionResponse,
    pub receipt: SelectionResponse,
    pub summary: SelectionResponse,
}

struct Budget {
    remaining: SelectionLimits,
}

impl Budget {
    const fn new(limits: SelectionLimits) -> Result<Self, SelectionError> {
        if limits.selections < 4 {
            return Err(SelectionError::LimitExceeded("selections"));
        }
        if limits.targets < 4 {
            return Err(SelectionError::LimitExceeded("targets"));
        }
        Ok(Self { remaining: limits })
    }

    fn source(&mut self, work: usize) -> Result<(), SelectionError> {
        self.remaining.source_bytes = self
            .remaining
            .source_bytes
            .checked_sub(work)
            .ok_or(SelectionError::LimitExceeded("source bytes"))?;
        Ok(())
    }

    fn response(
        &mut self,
        response: &SelectionResponse,
        work: usize,
    ) -> Result<(), SelectionError> {
        self.source(work)?;
        let bytes = encoded_size(response, self.remaining.response_bytes, "response bytes")?;
        self.remaining.response_bytes -= bytes;
        let proof = response.multiproof.as_ref().ok_or(SelectionError::InvalidProof)?;
        let hashes = proof
            .targets
            .len()
            .checked_add(proof.helpers.len())
            .ok_or(SelectionError::LimitExceeded("witness hashes"))?;
        self.remaining.witness_hashes = self
            .remaining
            .witness_hashes
            .checked_sub(hashes)
            .ok_or(SelectionError::LimitExceeded("witness hashes"))?;
        Ok(())
    }

    fn transaction_hash(&mut self, response: &SelectionResponse) -> Result<B256, SelectionError> {
        let bytes = one_value(response)?;
        let work = bytes
            .len()
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(SelectionError::LimitExceeded("source bytes"))?;
        self.source(work)?;
        decode_transaction(bytes)
            .and_then(|transaction| transaction.original_transaction_hash())
            .map_err(|_| SelectionError::InvalidValue)
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), SelectionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    Ok(())
}

pub(crate) fn check_request(
    request: &TransactionReceiptRequest,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<(), SelectionError> {
    check_cancelled(cancelled)?;
    encoded_size(request, limits.request_bytes, "request bytes")?;
    Budget::new(limits)?;
    Ok(())
}

fn item_request(index: u64) -> SelectionRequest {
    SelectionRequest {
        include_proof: true,
        selections: vec![ReceiptSelection {
            path: format!("[{index}]"),
            operation: SelectionOperation::Whole {},
        }],
    }
}

fn summary_request() -> SelectionRequest {
    SelectionRequest {
        include_proof: true,
        selections: [".transactions_root", ".receipts_root"]
            .into_iter()
            .map(|path| ReceiptSelection {
                path: path.into(),
                operation: SelectionOperation::Value {},
            })
            .collect(),
    }
}

fn one_value(response: &SelectionResponse) -> Result<&[u8], SelectionError> {
    let [result] = response.results.as_slice() else {
        return Err(SelectionError::InvalidValue);
    };
    let [value] = result.values_ssz.as_slice() else {
        return Err(SelectionError::InvalidValue);
    };
    Ok(value)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn associate(
    snapshot: &SourcePayloadSnapshot,
    request: &TransactionReceiptRequest,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<TransactionReceiptResponse, QueryError> {
    check_request(request, limits, cancelled)?;
    let source = snapshot.source();
    if source.native_block.hash != request.block_hash {
        return Err(QueryError::Provider(LookupError::UnknownBlock));
    }
    if source.root_context != RootContext::RethExperimentalUnanchored {
        return Err(QueryError::InvalidSnapshot);
    }
    select_payload(
        snapshot.payload(),
        request,
        block_status(source.canonicality),
        limits,
        cancelled,
    )
    .map_err(Into::into)
}

pub(crate) fn select_payload(
    payload: &ExecutionPayloadSnapshot,
    request: &TransactionReceiptRequest,
    status: &str,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<TransactionReceiptResponse, SelectionError> {
    check_request(request, limits, cancelled)?;
    let mut budget = Budget::new(limits)?;
    let (summary, work) = select_component_with_usage(
        ComponentSnapshot::ExecutionSummary(payload.summary()),
        &summary_request(),
        budget.remaining,
        cancelled,
    )?;
    budget.response(&summary.selection, work)?;
    let (transaction, work) = select_component_with_usage(
        ComponentSnapshot::Transactions(&payload.components().transactions),
        &item_request(request.transaction_index),
        budget.remaining,
        cancelled,
    )?;
    budget.response(&transaction.selection, work)?;
    check_cancelled(cancelled)?;
    let transaction_hash = budget.transaction_hash(&transaction.selection)?;
    check_cancelled(cancelled)?;
    let (receipt, work) = select_receipt_with_usage(
        &payload.components().receipts,
        request.transaction_index,
        budget.remaining,
        cancelled,
    )?;
    budget.response(&receipt, work)?;
    check_cancelled(cancelled)?;
    let response = TransactionReceiptResponse {
        block_hash: request.block_hash,
        transaction_index: request.transaction_index,
        root_context: "reth_experimental_unanchored".into(),
        block_status: status.into(),
        transaction_hash,
        transaction: transaction.selection,
        receipt,
        summary: summary.selection,
    };
    encoded_size(&response, limits.response_bytes, "response bytes")?;
    Ok(response)
}

pub fn verify_transaction_receipt(
    request: &TransactionReceiptRequest,
    response: &TransactionReceiptResponse,
    expected_summary_root: B256,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    verify_transaction_receipt_with_cancel(
        request,
        response,
        expected_summary_root,
        limits,
        &AtomicBool::new(false),
    )
}

pub fn verify_transaction_receipt_with_cancel(
    request: &TransactionReceiptRequest,
    response: &TransactionReceiptResponse,
    expected_summary_root: B256,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<(), SelectionError> {
    check_request(request, limits, cancelled)?;
    encoded_size(response, limits.response_bytes, "response bytes")?;
    if response.block_hash != request.block_hash ||
        response.transaction_index != request.transaction_index ||
        response.root_context != "reth_experimental_unanchored" ||
        !matches!(response.block_status.as_str(), "canonical" | "non_canonical" | "unknown")
    {
        return Err(SelectionError::InvalidContext);
    }
    let mut budget = Budget::new(limits)?;
    let work = verify_component_response_with_usage(
        ComponentObject::ExecutionSummary,
        &summary_request(),
        &response.summary,
        expected_summary_root,
        budget.remaining,
    )?;
    budget.response(&response.summary, work)?;
    let [transactions, receipts] = response.summary.results.as_slice() else {
        return Err(SelectionError::InvalidValue);
    };
    let root = |result: &crate::SelectionResult| -> Result<B256, SelectionError> {
        let [value] = result.values_ssz.as_slice() else {
            return Err(SelectionError::InvalidValue);
        };
        if value.len() != 32 {
            return Err(SelectionError::InvalidValue);
        }
        Ok(B256::from_slice(value))
    };
    check_cancelled(cancelled)?;
    let work = verify_component_response_with_usage(
        ComponentObject::Transactions,
        &item_request(request.transaction_index),
        &response.transaction,
        root(transactions)?,
        budget.remaining,
    )?;
    budget.response(&response.transaction, work)?;
    check_cancelled(cancelled)?;
    if budget.transaction_hash(&response.transaction)? != response.transaction_hash {
        return Err(SelectionError::InvalidValue);
    }
    check_cancelled(cancelled)?;
    let value = one_value(&response.receipt)?;
    let mut work = receipt_verification_work(value, budget.remaining.source_bytes)?;
    let proof = response.receipt.multiproof.as_ref().ok_or(SelectionError::InvalidProof)?;
    add(
        &mut work,
        multiproof::proof_work(proof, budget.remaining)?,
        budget.remaining.source_bytes,
        "source bytes",
    )?;
    add(
        &mut work,
        value.len().checked_mul(2).ok_or(SelectionError::LimitExceeded("source bytes"))?,
        budget.remaining.source_bytes,
        "source bytes",
    )?;
    verify_receipt_selection(
        &item_request(request.transaction_index),
        &response.receipt,
        root(receipts)?,
        budget.remaining,
    )?;
    budget.response(&response.receipt, work)?;
    check_cancelled(cancelled)
}

#[cfg(test)]
mod tests;
