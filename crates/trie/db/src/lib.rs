//! An integration of `reth-trie` with `reth-db`.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod hashed_cursor;
mod prefix_set;
mod proof;
mod state;
mod storage;
mod trie_cursor;
mod witness;

pub use hashed_cursor::{
    DatabaseHashedAccountCursor, DatabaseHashedCursorFactory, DatabaseHashedStorageCursor,
};
pub use prefix_set::PrefixSetLoader;
pub use proof::{
    overlay_storage_multiproof_from_nodes, overlay_storage_proof_from_nodes, DatabaseProof,
    DatabaseStorageProof,
};
pub use state::{DatabaseHashedPostState, DatabaseStateRoot};
pub use storage::{overlay_storage_root_from_nodes, DatabaseHashedStorage, DatabaseStorageRoot};
pub use trie_cursor::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
};
pub use witness::DatabaseTrieWitness;
