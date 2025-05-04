//! The implementation of sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod state;
pub use state::*;

mod trie;
pub use trie::*;

pub mod blinded;

mod metrics;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
