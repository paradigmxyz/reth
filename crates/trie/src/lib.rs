#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! The implementation of Merkle Patricia Trie, a cryptographically
//! authenticated radix trie that is used to store key-value bindings.
//! <https://ethereum.org/en/developers/docs/data-structures-and-encoding/patricia-merkle-trie/>

mod nibbles;
pub use nibbles::Nibbles;

/// The Ethereum account as represented in the trie.
pub mod account;

/// Various branch nodes producde by the hash builder.
pub mod nodes;

/// The implementation of hash builder.
pub mod hash_builder;

/// The implementation of a container for storing intermediate changes to a trie.
/// The container indicates when the trie has been modified.
pub mod prefix_set;

/// The cursor implementations for navigating account and storage tries.
pub mod trie_cursor;

/// The cursor implementations for navigating hashed state.
pub mod hashed_cursor;

/// The trie walker for iterating over the trie nodes.
pub mod walker;

mod errors;
pub use errors::{StateRootError, StorageRootError};

/// The implementation of the Merkle Patricia Trie.
mod trie;
pub use trie::{StateRoot, StorageRoot};

/// Buffer for trie updates.
pub mod updates;

/// Utilities for state root checkpoint progress.
mod progress;
pub use progress::{IntermediateStateRootState, StateRootProgress};

/// Collection of trie-related test utilities.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
