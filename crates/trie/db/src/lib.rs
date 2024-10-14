//! An integration of [`reth-trie`] with [`reth-db`].

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
pub use proof::DatabaseProof;
use reth_db::transaction::DbTx;
use reth_trie::{proof::Proof, witness::TrieWitness, KeccakKeyHasher, StateRoot, StorageRoot};
pub use state::{DatabaseHashedPostState, DatabaseStateRoot};
pub use storage::{DatabaseHashedStorage, DatabaseStorageRoot};
pub use trie_cursor::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
};
pub use witness::DatabaseTrieWitness;

/// Database state trait.
pub trait DatabaseState: std::fmt::Debug + Send + Sync + Unpin + 'static {
    /// The state root type.
    type StateRoot<'a, TX: DbTx + 'a>: DatabaseStateRoot<'a, TX>;
    /// The storage root type.
    type StorageRoot<'a, TX: DbTx + 'a>: DatabaseStorageRoot<'a, TX>;
    /// The state proof type.
    type StateProof<'a, TX: DbTx + 'a>: DatabaseProof<'a, TX>;
    /// The state witness type.
    type StateWitness<'a, TX: DbTx + 'a>: DatabaseTrieWitness<'a, TX>;
    /// The key hasher type.
    type KeyHasher: reth_trie::KeyHasher;
}

impl DatabaseState for () {
    type StateRoot<'a, TX: DbTx + 'a> = StateRoot<
        DatabaseTrieCursorFactory<'a, TX>,
        DatabaseHashedCursorFactory<'a, TX>,
        Self::KeyHasher,
    >;
    type StorageRoot<'a, TX: DbTx + 'a> = StorageRoot<
        DatabaseTrieCursorFactory<'a, TX>,
        DatabaseHashedCursorFactory<'a, TX>,
        Self::KeyHasher,
    >;
    type StateProof<'a, TX: DbTx + 'a> = Proof<
        DatabaseTrieCursorFactory<'a, TX>,
        DatabaseHashedCursorFactory<'a, TX>,
        Self::KeyHasher,
    >;
    type StateWitness<'a, TX: DbTx + 'a> = TrieWitness<
        DatabaseTrieCursorFactory<'a, TX>,
        DatabaseHashedCursorFactory<'a, TX>,
        Self::KeyHasher,
    >;
    type KeyHasher = KeccakKeyHasher;
}
