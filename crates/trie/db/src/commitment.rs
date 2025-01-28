use crate::{
    DatabaseHashedCursorFactory, DatabaseProof, DatabaseStateRoot, DatabaseStorageRoot,
    DatabaseTrieCursorFactory, DatabaseTrieWitness,
};
use reth_db::transaction::DbTx;
use reth_trie::{
    proof::Proof, witness::TrieWitness, KeccakKeyHasher, KeyHasher, StateRoot, StorageRoot,
};

/// The `StateCommitment` trait provides associated types for state commitment operations.
pub trait StateCommitment: std::fmt::Debug + Send + Sync + Unpin + 'static {
    /// The state root type.
    type StateRoot<'a, TX: DbTx + 'a>: DatabaseStateRoot<'a, TX>;
    /// The storage root type.
    type StorageRoot<'a, TX: DbTx + 'a>: DatabaseStorageRoot<'a, TX>;
    /// The state proof type.
    type StateProof<'a, TX: DbTx + 'a>: DatabaseProof<'a, TX>;
    /// The state witness type.
    type StateWitness<'a, TX: DbTx + 'a>: DatabaseTrieWitness<'a, TX>;
    /// The key hasher type.
    type KeyHasher: KeyHasher;
}

/// The state commitment type for Ethereum's Merkle Patricia Trie.
#[derive(Debug)]
#[non_exhaustive]
pub struct MerklePatriciaTrie;

impl StateCommitment for MerklePatriciaTrie {
    type StateRoot<'a, TX: DbTx + 'a> =
        StateRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>;
    type StorageRoot<'a, TX: DbTx + 'a> =
        StorageRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>;
    type StateProof<'a, TX: DbTx + 'a> =
        Proof<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>;
    type StateWitness<'a, TX: DbTx + 'a> =
        TrieWitness<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>;
    type KeyHasher = KeccakKeyHasher;
}
