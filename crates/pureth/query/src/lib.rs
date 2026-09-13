#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

mod path;
mod proof;
mod proof_access;
mod query_service;
mod receipt_resolution;
#[cfg(not(target_arch = "wasm32"))]
mod rpc;
mod schema;
#[cfg(test)]
mod vector;
#[cfg(test)]
mod vector_records;

pub use path::{parse_path, ParseError, PathToken};
pub use proof::{
    address_target_node, verify_branch, verify_receipt_log_address, EnvelopeError,
    InvalidAddressLength, ProofError,
};
pub use proof_access::{prove_receipt_log_address, ProofAccessError, ReceiptLogAddressProof};
pub use query_service::{
    verify_query_response, QueryError, QueryRequest, QueryResponse, QueryService,
    ResponseVerificationError,
};
pub use receipt_resolution::{resolve_receipt_log_address, ReceiptResolutionError, ReceiptsSsz};
#[cfg(not(target_arch = "wasm32"))]
pub use rpc::{PurethApiServer, PurethRpc};
pub use schema::{
    branch_positions, compose_gindices, container_field_gindex, progressive_chunk_gindex,
    receipt_log_address_gindex, resolve, validate_runtime_bounds, BoundsError, GindexError,
    ResolvedPath, UnsupportedPath, SCHEMA_ID,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod vector_tests;
