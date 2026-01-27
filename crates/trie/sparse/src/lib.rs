//! The implementation of sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// Default depth to prune sparse tries to for cross-payload caching.
pub const DEFAULT_SPARSE_TRIE_PRUNE_DEPTH: usize = 4;

/// Default number of storage tries to preserve across payload validations.
pub const DEFAULT_MAX_PRESERVED_STORAGE_TRIES: usize = 100;

mod state;
pub use state::*;

mod trie;
pub use trie::*;

mod traits;
pub use traits::*;

pub mod provider;

#[cfg(feature = "metrics")]
mod metrics;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
