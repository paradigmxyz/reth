//! Collection of trie related types.

/// The implementation of hash builder.
pub mod hash_builder;

mod account;
pub use account::TrieAccount;

mod mask;
pub(crate) use mask::StoredTrieMask;

mod nibbles;
pub use nibbles::{Nibbles, StoredNibbles, StoredNibblesSubKey};

pub mod nodes;
pub use nodes::StoredBranchNode;

mod proofs;
pub use proofs::{AccountProof, StorageProof};

mod storage;
pub use storage::StorageTrieEntry;

mod subnode;
pub use subnode::StoredSubNode;

pub use alloy_trie::{BranchNodeCompact, HashBuilder, TrieMask, EMPTY_ROOT_HASH};
