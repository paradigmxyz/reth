use super::{PoseidonKeyHasher, StateRoot, StorageRoot};
use reth_db::transaction::DbTx;
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, StateCommitment};

/// The state commitment type for Scroll's binary Merkle Patricia Trie.
#[derive(Debug)]
#[non_exhaustive]
pub struct BinaryMerklePatriciaTrie;

impl StateCommitment for BinaryMerklePatriciaTrie {
    type KeyHasher = PoseidonKeyHasher;
    type StateRoot<'a, TX: DbTx + 'a> =
        StateRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>;
    type StorageRoot<'a, TX: DbTx + 'a> =
        StorageRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>;
    // TODO(scroll): replace with scroll proof type
    type StateProof<'a, TX: DbTx + 'a> = reth_trie::proof::Proof<
        DatabaseTrieCursorFactory<'a, TX>,
        DatabaseHashedCursorFactory<'a, TX>,
    >;
    // TODO(scroll): replace with scroll witness type
    type StateWitness<'a, TX: DbTx + 'a> = reth_trie::witness::TrieWitness<
        DatabaseTrieCursorFactory<'a, TX>,
        DatabaseHashedCursorFactory<'a, TX>,
    >;
}
