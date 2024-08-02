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

/// The implementation of forward-only in-memory cursor.
pub mod forward_cursor;

/// The cursor implementations for navigating account and storage tries.
pub mod trie_cursor;

/// The cursor implementations for navigating hashed state.
pub mod hashed_cursor;

/// The trie walker for iterating over the trie nodes.
pub mod walker;

/// The iterators for traversing existing intermediate hashes and updated trie leaves.
pub mod node_iter;

/// In-memory hashed state.
mod state;
pub use state::*;

/// Merkle proof generation.
pub mod proof;

/// Trie witness generation.
pub mod witness;

/// The implementation of the Merkle Patricia Trie.
mod trie;
pub use trie::{StateRoot, StorageRoot};

/// Buffer for trie updates.
pub mod updates;

/// Utilities for state root checkpoint progress.
mod progress;
pub use progress::{IntermediateStateRootState, StateRootProgress};

/// Trie calculation stats.
pub mod stats;

// re-export for convenience
pub use reth_trie_common::*;

/// Trie calculation metrics.
#[cfg(feature = "metrics")]
pub mod metrics;

/// Collection of trie-related test utilities.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
