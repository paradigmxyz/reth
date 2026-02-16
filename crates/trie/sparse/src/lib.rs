//! The implementation of sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod state;
pub use state::*;

mod trie;
pub use trie::*;

mod traits;
pub use traits::*;

mod parallel;
pub use parallel::*;

mod lower;

pub mod provider;

#[cfg(feature = "metrics")]
mod metrics;

#[cfg(feature = "trie-debug")]
pub mod debug_recorder;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
