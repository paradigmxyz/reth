#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

mod path;
mod proof;
mod schema;
mod vector;
mod vector_bundle;
mod vector_records;

pub use path::{parse_path, ParseError, PathToken};
pub use proof::{
    address_target_node, verify_branch, verify_receipt_log_address, EnvelopeError,
    InvalidAddressLength, ProofError,
};
pub use schema::{
    branch_positions, compose_gindices, container_field_gindex, progressive_chunk_gindex,
    receipt_log_address_gindex, resolve_v0, validate_runtime_bounds, BoundsError, GindexError,
    ResolvedPath, UnsupportedPath, SCHEMA_DIGEST, SCHEMA_ID,
};
pub use vector::{FirstTarget, LogInput, ReceiptFixture, ReceiptInput, VectorError};
pub use vector_bundle::{load_manifest_proof_case, load_manifest_root_case};
pub use vector_records::{LoadedFixture, LoadedProofCase, LoadedRootCase, ProofRecord, RootRecord};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod vector_tests;
