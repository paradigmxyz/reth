#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

mod association;
mod components;
mod path;
mod proof;
mod proof_access;
mod query_service;
mod receipt_response;
#[cfg(not(target_arch = "wasm32"))]
mod rpc;
mod schema;
mod selection;
#[cfg(not(target_arch = "wasm32"))]
mod selection_executor;

pub use association::{
    verify_transaction_receipt, verify_transaction_receipt_with_cancel, TransactionReceiptRequest,
    TransactionReceiptResponse,
};
pub use components::{
    link_component, select_component, select_component_with_cancel, verify_component_link,
    verify_component_selection, ComponentLinkProof, ComponentObject, ComponentResponse,
    ComponentSnapshot,
};
pub use path::{parse_path, ParseError, PathToken};
pub use proof::{verify_branch, ProofError};
pub use proof_access::ProofAccessError;
#[cfg(not(target_arch = "wasm32"))]
pub use query_service::RethPayloadProvider;
pub use query_service::{
    query_snapshot, verify_query_response, QueryError, QueryHandler, QueryRequest, QueryResponse,
    QueryService, ResponseVerificationError,
};
#[cfg(not(target_arch = "wasm32"))]
pub use receipt_response::{
    convert_eip6466_block_receipts, convert_eip6466_block_receipts_from_retained,
    convert_eip6466_receipt, convert_eip6466_receipt_from_retained,
    verify_eip6466_receipt_response, ExecutionHeaderConverter, ExecutionReceiptConverter,
    ExecutionReceiptEnvelope, ExecutionRpcReceipt, ExecutionRpcTypes,
};
pub use receipt_response::{
    extend_block_receipt_responses, extend_receipt_response, verify_whole_receipt_proof,
    ReceiptProofSupport, ReceiptResponse, WholeReceiptProof,
};
#[cfg(not(target_arch = "wasm32"))]
pub use rpc::{PurethApiServer, PurethRpc};
pub use schema::{
    branch_positions, compose_gindices, container_field_gindex, progressive_chunk_gindex,
    GindexError,
};
pub use selection::{
    decode_selection_request, select_receipt_multiproof, select_receipt_snapshot,
    verify_receipt_selection, FilterMatch, FilterOutput, FilterPredicate, ReceiptSelection,
    SelectionError, SelectionLimits, SelectionMultiproof, SelectionOperation, SelectionProofFormat,
    SelectionRequest, SelectionResponse, SelectionResult, SelectionWitness,
};
#[cfg(not(target_arch = "wasm32"))]
pub use selection_executor::SelectionExecutor;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_utils;
