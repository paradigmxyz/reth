use alloy_primitives::B256;
use reth_db_api::{cursor::DbDupCursorRO, transaction::DbTx};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{BranchNodeCompact, Nibbles};
use reth_trie_db::{StorageTrieEntryLike, TrieTableAdapter};

/// Returns the compact storage trie marker that retains only the storage root commitment.
pub(crate) fn node(storage_root: B256) -> BranchNodeCompact {
    BranchNodeCompact::new(0, 0, 0, Vec::new(), Some(storage_root))
}

/// Returns true if the node is an account-storage prune marker.
pub(crate) fn is_node(node: &BranchNodeCompact) -> bool {
    node.state_mask.is_empty() &&
        node.tree_mask.is_empty() &&
        node.hash_mask.is_empty() &&
        node.hashes.is_empty() &&
        node.root_hash.is_some()
}

/// Reads the retained storage root marker for `hashed_address`, if present.
pub(crate) fn read<TX, A>(tx: &TX, hashed_address: B256) -> ProviderResult<Option<B256>>
where
    TX: DbTx,
    A: TrieTableAdapter,
{
    let empty_path = A::StorageSubKey::from(Nibbles::default());
    let mut cursor = tx.cursor_dup_read::<A::StorageTrieTable>()?;

    Ok(cursor
        .seek_by_key_subkey(hashed_address, empty_path.clone())?
        .filter(|entry| entry.nibbles() == &empty_path && is_node(entry.node()))
        .and_then(|entry| entry.node().root_hash))
}
