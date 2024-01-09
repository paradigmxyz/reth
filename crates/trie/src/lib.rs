//! The implementation of Merkle Patricia Trie, a cryptographically
//! authenticated radix trie that is used to store key-value bindings.
//! <https://ethereum.org/en/developers/docs/data-structures-and-encoding/patricia-merkle-trie/>
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

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
pub use errors::*;

// The iterators for traversing existing intermediate hashes and updated trie leaves.
pub(crate) mod node_iter;

/// Merkle proof generation.
pub mod proof;

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
