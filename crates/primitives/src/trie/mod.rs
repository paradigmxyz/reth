//! Collection of trie related types.

/// Various branch nodes produced by the hash builder.
pub mod nodes;
pub use nodes::BranchNodeCompact;

/// The implementation of hash builder.
pub mod hash_builder;
pub use hash_builder::HashBuilder;

/// Merkle trie proofs.
mod proofs;
pub use proofs::{AccountProof, StorageProof};

mod account;
mod mask;
mod nibbles;
mod storage;
mod subnode;

pub use self::{
    account::TrieAccount,
    mask::TrieMask,
    nibbles::{Nibbles, StoredNibblesSubKey},
    storage::StorageTrieEntry,
    subnode::StoredSubNode,
};
