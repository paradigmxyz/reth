//! The implementation of sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod state;
pub use state::*;

mod trie;
pub use trie::*;

pub mod blinded;

mod nibbles;
pub use nibbles::*;

mod prefix_set;
pub use prefix_set::*;

mod node;
pub use node::*;

#[cfg(feature = "metrics")]
mod metrics;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
