//! Commonly used types for trie usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// The implementation of hash builder.
pub mod hash_builder;

/// Constants related to the trie computation.
mod constants;
pub use constants::*;

mod account;
pub use account::TrieAccount;

mod key;
pub use key::{KeccakKeyHasher, KeyHasher};

mod nibbles;
pub use nibbles::{Nibbles, StoredNibbles, StoredNibblesSubKey};

mod storage;
pub use storage::StorageTrieEntry;

mod subnode;
pub use subnode::StoredSubNode;

/// The implementation of a container for storing intermediate changes to a trie.
/// The container indicates when the trie has been modified.
pub mod prefix_set;

mod proofs;
#[cfg(any(test, feature = "test-utils"))]
pub use proofs::triehash;
pub use proofs::*;

pub mod root;

/// Buffer for trie updates.
pub mod updates;

/// Bincode-compatible serde implementations for trie types.
///
/// `bincode` crate allows for more efficient serialization of trie types, because it allows
/// non-string map keys.
///
/// Read more: <https://github.com/paradigmxyz/reth/issues/11370>
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub mod serde_bincode_compat {
    pub use super::updates::serde_bincode_compat as updates;
}

/// Re-export
pub use alloy_trie::{nodes::*, proof, BranchNodeCompact, HashBuilder, TrieMask, EMPTY_ROOT_HASH};
