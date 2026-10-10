#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

mod path;
mod proof;
mod proof_access;
mod query_service;
#[cfg(not(target_arch = "wasm32"))]
mod rpc;
mod schema;
mod selection;
#[cfg(not(target_arch = "wasm32"))]
mod selection_executor;

pub use path::{parse_path, ParseError, PathToken};
pub use proof::{address_target_node, verify_branch, InvalidAddressLength, ProofError};
pub use proof_access::ProofAccessError;
pub use query_service::{
    query_snapshot, verify_query_response, QueryError, QueryHandler, QueryRequest, QueryResponse,
    QueryService, ResponseVerificationError,
};
#[cfg(not(target_arch = "wasm32"))]
pub use rpc::{PurethApiServer, PurethRpc};
pub use schema::{
    branch_positions, compose_gindices, container_field_gindex, progressive_chunk_gindex,
    GindexError,
};
pub use selection::{
    decode_selection_request, select_receipt_snapshot, verify_receipt_selection, ReceiptSelection,
    SelectionError, SelectionLimits, SelectionOperation, SelectionProofFormat, SelectionRequest,
    SelectionResponse, SelectionResult, SelectionWitness,
};
#[cfg(not(target_arch = "wasm32"))]
pub use selection_executor::SelectionExecutor;

#[cfg(test)]
mod tests;
