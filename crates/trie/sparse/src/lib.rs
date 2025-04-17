//! The implementation of sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc as std;

/// A macro that expands to a trace! call when std feature is enabled,
/// and does nothing when `no_std` is enabled.
#[macro_export]
macro_rules! trace {
    ($($args:tt)*) => {
        #[cfg(feature = "std")]
        reth_tracing::tracing::trace!($($args)*);
        #[cfg(not(feature = "std"))]
        {
            // Currently in no_std, trace! is a noop
        };
    };
}

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
