//! The implementation of sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
mod state;
#[cfg(feature = "std")]
pub use state::*;

#[cfg(feature = "std")]
mod trie;
#[cfg(feature = "std")]
pub use trie::*;

mod traits;
pub use traits::*;

#[cfg(feature = "std")]
mod arena;
#[cfg(feature = "std")]
pub use arena::*;

#[cfg(feature = "metrics")]
mod metrics;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
