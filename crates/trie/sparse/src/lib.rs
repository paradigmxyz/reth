//! The implementation of sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod blinded;

mod node;
pub use node::*;

mod parallel_trie;
pub use parallel_trie::*;

mod rlp;
pub use rlp::*;

mod state;
pub use state::*;

mod trie;
pub use trie::*;

mod updates;
pub use updates::*;

#[cfg(feature = "metrics")]
mod metrics;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
