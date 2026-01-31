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

pub mod provider;

#[cfg(feature = "std")]
pub mod hot_accounts;
#[cfg(feature = "std")]
pub use hot_accounts::{
    BloomFilter, HotAccountConfig, HotStorageSlots, Hotness, RotatingBloomFilter, SmartPruneConfig,
    StorageSlotKey, TieredHotAccounts,
};

#[cfg(feature = "metrics")]
mod metrics;

/// Re-export sparse trie error types.
pub mod errors {
    pub use reth_execution_errors::{
        SparseStateTrieError, SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieError,
        SparseTrieErrorKind, SparseTrieResult,
    };
}
