#![allow(dead_code)]

use alloy_primitives::B256;
use reth_db::{
    mdbx::{DatabaseArguments, SyncMode},
    DatabaseEnv, DatabaseEnvKind,
};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_trie::{updates::StorageTrieUpdatesSorted, BranchNodeCompact, Nibbles};
use reth_trie_db::{StorageTrieEntryLike, TrieTableAdapter};
use std::{collections::BTreeMap, path::Path};

pub(crate) fn open_database(path: &Path) -> DatabaseEnv {
    let mut db = DatabaseEnv::open(
        path,
        DatabaseEnvKind::RW,
        DatabaseArguments::test().with_sync_mode(Some(SyncMode::Durable)),
    )
    .unwrap();
    db.create_tables().unwrap();
    db
}

pub(crate) fn node(seed: u64, hashes: u8) -> BranchNodeCompact {
    let mask = ((1u32 << hashes) - 1) as u16;
    BranchNodeCompact::new(
        u16::MAX,
        mask,
        mask,
        (0..hashes)
            .map(|index| {
                let mut hash = B256::repeat_byte(index);
                hash[..8].copy_from_slice(&seed.to_be_bytes());
                hash
            })
            .collect(),
        Some(B256::with_last_byte(seed as u8)),
    )
}

pub(crate) fn path(index: u32) -> Nibbles {
    Nibbles::unpack(index.to_be_bytes())
}

/// The pre-optimization implementation, retained as the benchmark's reference writer.
pub(crate) fn baseline_write<C, A>(
    cursor: &mut C,
    address: B256,
    updates: &StorageTrieUpdatesSorted,
) -> Result<usize, DatabaseError>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable>
        + DbCursorRW<A::StorageTrieTable>
        + DbDupCursorRO<A::StorageTrieTable>
        + DbDupCursorRW<A::StorageTrieTable>,
{
    let mut count = 0;
    for (path, update) in updates.storage_nodes.iter().filter(|(path, _)| !path.is_empty()) {
        count += 1;
        let subkey = A::StorageSubKey::from(*path);
        if cursor
            .seek_by_key_subkey(address, subkey.clone())?
            .as_ref()
            .is_some_and(|entry| *entry.nibbles() == subkey)
        {
            cursor.delete_current()?;
        }
        if let Some(node) = update {
            cursor.upsert(address, &A::StorageValue::new(subkey, node.clone()))?;
        }
    }
    Ok(count)
}

pub(crate) fn snapshot<A: TrieTableAdapter>(
    tx: &impl DbTx,
) -> BTreeMap<(B256, Nibbles), BranchNodeCompact> {
    tx.cursor_read::<A::StorageTrieTable>()
        .unwrap()
        .walk(None)
        .unwrap()
        .map(|entry| {
            let (address, entry) = entry.unwrap();
            let (subkey, node) = entry.into_parts();
            ((address, A::subkey_to_nibbles(&subkey)), node)
        })
        .collect()
}

pub(crate) fn wipe<A: TrieTableAdapter>(tx: &impl DbTxMut, address: B256) {
    let mut cursor = tx.cursor_dup_write::<A::StorageTrieTable>().unwrap();
    if cursor.seek_exact(address).unwrap().is_some() {
        cursor.delete_current_duplicates().unwrap();
    }
}
