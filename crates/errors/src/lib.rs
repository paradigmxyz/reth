//! High level error types for the reth in general.
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod error;
pub use error::{RethError, RethResult};

pub use reth_blockchain_tree_api::error::{BlockchainTreeError, CanonicalError};
pub use reth_consensus::ConsensusError;
pub use reth_execution_errors::{BlockExecutionError, BlockValidationError};
pub use reth_storage_errors::{
    db::DatabaseError,
    provider::{ProviderError, ProviderResult},
};
