use alloy_primitives::B256;
use reth_db_api::{cursor::DbDupCursorRO, transaction::DbTx};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::Nibbles;
use reth_trie_db::{StorageTrieEntryLike, TrieTableAdapter};

pub(crate) use reth_trie::storage_root_marker::{is_node, node};

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
