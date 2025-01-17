//! An integration of [`reth-trie`] with [`reth-db`].

use reth_db::transaction::DbTx;

mod commitment;
mod hashed_cursor;
mod prefix_set;
mod proof;
mod state;
mod storage;
mod trie_cursor;
mod witness;

pub use commitment::{MerklePatriciaTrie, StateCommitment};
pub use hashed_cursor::{
    DatabaseHashedAccountCursor, DatabaseHashedCursorFactory, DatabaseHashedStorageCursor,
};
pub use prefix_set::PrefixSetLoader;
pub use proof::{DatabaseProof, DatabaseStorageProof};
pub use state::{DatabaseHashedPostState, DatabaseStateRoot};
pub use storage::{DatabaseHashedStorage, DatabaseStorageRoot};
pub use trie_cursor::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
};
pub use witness::DatabaseTrieWitness;

/// Trait for database operations needed by the trie cursor factories
pub trait DatabaseRef: Send + Sync {
    /// The concrete transaction type
    type Tx: DbTx;

    /// Returns a reference to the underlying database transaction
    fn tx_reference(&self) -> &Self::Tx;
}

impl<T: DbTx> DatabaseRef for &'_ T {
    type Tx = T;

    fn tx_reference(&self) -> &Self::Tx {
        self
    }
}
