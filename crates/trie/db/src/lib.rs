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
pub use prefix_set::{load_prefix_sets_with_provider, PrefixSetLoader};
pub use proof::{DatabaseProof, DatabaseStorageProof};
pub use state::{
    hashed_post_state_from_reverts_with_provider, DatabaseHashedPostState, DatabaseStateRoot,
};
pub use storage::{DatabaseHashedStorage, DatabaseStorageRoot};
pub use trie_cursor::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
};
pub use witness::DatabaseTrieWitness;
