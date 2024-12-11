//! The implementation of sparse MPT.

mod state;
pub use state::*;

mod trie;
pub use trie::*;

pub mod blinded;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
