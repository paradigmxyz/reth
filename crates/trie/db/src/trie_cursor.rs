use alloy_primitives::B256;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::StorageTrieUpdatesSorted,
    BranchNodeCompact, Nibbles, PackedStorageTrieEntry, PackedStoredNibbles,
    PackedStoredNibblesSubKey, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey,
};

/// Wrapper struct for database transaction implementing trie cursor factory trait.
#[derive(Debug, Clone)]
pub struct DatabaseTrieCursorFactory<T>(T);

impl<T> DatabaseTrieCursorFactory<T> {
    /// Create new [`DatabaseTrieCursorFactory`].
    pub const fn new(tx: T) -> Self {
        Self(tx)
    }
}

impl<TX> TrieCursorFactory for DatabaseTrieCursorFactory<&TX>
where
    TX: DbTx,
{
    type AccountTrieCursor<'a>
        = DatabaseAccountTrieCursor<<TX as DbTx>::Cursor<tables::AccountsTrie>>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = DatabaseStorageTrieCursor<<TX as DbTx>::DupCursor<tables::StoragesTrie>>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseAccountTrieCursor::new(self.0.cursor_read::<tables::AccountsTrie>()?))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseStorageTrieCursor::new(
            self.0.cursor_dup_read::<tables::StoragesTrie>()?,
            hashed_address,
        ))
    }
}

/// A cursor over the account trie.
#[derive(Debug)]
pub struct DatabaseAccountTrieCursor<C>(pub(crate) C);

impl<C> DatabaseAccountTrieCursor<C> {
    /// Create a new account trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> TrieCursor for DatabaseAccountTrieCursor<C>
where
    C: DbCursorRO<tables::AccountsTrie> + Send,
{
    /// Seeks an exact match for the provided key in the account trie.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek_exact(StoredNibbles(key))?.map(|value| (value.0 .0, value.1)))
    }

    /// Seeks a key in the account trie that matches or is greater than the provided key.
    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek(StoredNibbles(key))?.map(|value| (value.0 .0, value.1)))
    }

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.next()?.map(|value| (value.0 .0, value.1)))
    }

    /// Retrieves the current key in the cursor.
    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.0.current()?.map(|(k, _)| k.0))
    }

    fn reset(&mut self) {
        // No-op for database cursors
    }
}

/// A cursor over the storage tries stored in the database.
#[derive(Debug)]
pub struct DatabaseStorageTrieCursor<C> {
    /// The underlying cursor.
    pub cursor: C,
    /// Hashed address used for cursor positioning.
    hashed_address: B256,
}

impl<C> DatabaseStorageTrieCursor<C> {
    /// Create a new storage trie cursor.
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<C> DatabaseStorageTrieCursor<C>
where
    C: DbCursorRO<tables::StoragesTrie>
        + DbCursorRW<tables::StoragesTrie>
        + DbDupCursorRO<tables::StoragesTrie>
        + DbDupCursorRW<tables::StoragesTrie>,
{
    /// Writes storage updates that are already sorted
    pub fn write_storage_trie_updates_sorted(
        &mut self,
        updates: &StorageTrieUpdatesSorted,
    ) -> Result<usize, DatabaseError> {
        // The storage trie for this account has to be deleted.
        if updates.is_deleted() && self.cursor.seek_exact(self.hashed_address)?.is_some() {
            self.cursor.delete_current_duplicates()?;
        }

        let mut num_entries = 0;
        for (nibbles, maybe_updated) in updates.storage_nodes.iter().filter(|(n, _)| !n.is_empty())
        {
            num_entries += 1;
            let nibbles = StoredNibblesSubKey(*nibbles);
            // Delete the old entry if it exists.
            if self
                .cursor
                .seek_by_key_subkey(self.hashed_address, nibbles.clone())?
                .filter(|e| e.nibbles == nibbles)
                .is_some()
            {
                self.cursor.delete_current()?;
            }

            // There is an updated version of this node, insert new entry.
            if let Some(node) = maybe_updated {
                self.cursor.upsert(
                    self.hashed_address,
                    &StorageTrieEntry { nibbles, node: node.clone() },
                )?;
            }
        }

        Ok(num_entries)
    }
}

impl<C> TrieCursor for DatabaseStorageTrieCursor<C>
where
    C: DbCursorRO<tables::StoragesTrie> + DbDupCursorRO<tables::StoragesTrie> + Send,
{
    /// Seeks an exact match for the given key in the storage trie.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, StoredNibblesSubKey(key))?
            .filter(|e| e.nibbles == StoredNibblesSubKey(key))
            .map(|value| (value.nibbles.0, value.node)))
    }

    /// Seeks the given key in the storage trie.
    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, StoredNibblesSubKey(key))?
            .map(|value| (value.nibbles.0, value.node)))
    }

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.cursor.next_dup()?.map(|(_, v)| (v.nibbles.0, v.node)))
    }

    /// Retrieves the current value in the storage trie cursor.
    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.cursor.current()?.map(|(_, v)| v.nibbles.0))
    }

    fn reset(&mut self) {
        // No-op for database cursors
    }
}

impl<C> TrieStorageCursor for DatabaseStorageTrieCursor<C>
where
    C: DbCursorRO<tables::StoragesTrie> + DbDupCursorRO<tables::StoragesTrie> + Send,
{
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.hashed_address = hashed_address;
    }
}

// ── Storage v2: packed nibble encoding ─────────────────────────────────

/// Wrapper struct for database transaction implementing trie cursor factory trait
/// using packed nibble encoding (storage v2).
#[derive(Debug, Clone)]
pub struct PackedDatabaseTrieCursorFactory<T>(T);

impl<T> PackedDatabaseTrieCursorFactory<T> {
    /// Create new [`PackedDatabaseTrieCursorFactory`].
    pub const fn new(tx: T) -> Self {
        Self(tx)
    }
}

impl<TX> TrieCursorFactory for PackedDatabaseTrieCursorFactory<&TX>
where
    TX: DbTx,
{
    type AccountTrieCursor<'a>
        = PackedDatabaseAccountTrieCursor<<TX as DbTx>::Cursor<tables::AccountsTrieV2>>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = PackedDatabaseStorageTrieCursor<<TX as DbTx>::DupCursor<tables::StoragesTrieV2>>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(PackedDatabaseAccountTrieCursor::new(self.0.cursor_read::<tables::AccountsTrieV2>()?))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(PackedDatabaseStorageTrieCursor::new(
            self.0.cursor_dup_read::<tables::StoragesTrieV2>()?,
            hashed_address,
        ))
    }
}

/// A cursor over the account trie using packed nibble encoding (storage v2).
#[derive(Debug)]
pub struct PackedDatabaseAccountTrieCursor<C>(C);

impl<C> PackedDatabaseAccountTrieCursor<C> {
    /// Create a new packed account trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> TrieCursor for PackedDatabaseAccountTrieCursor<C>
where
    C: DbCursorRO<tables::AccountsTrieV2> + Send,
{
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek_exact(PackedStoredNibbles(key))?.map(|value| (value.0 .0, value.1)))
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek(PackedStoredNibbles(key))?.map(|value| (value.0 .0, value.1)))
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.next()?.map(|value| (value.0 .0, value.1)))
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.0.current()?.map(|(k, _)| k.0))
    }

    fn reset(&mut self) {}
}

/// A cursor over the storage tries using packed nibble encoding (storage v2).
#[derive(Debug)]
pub struct PackedDatabaseStorageTrieCursor<C> {
    /// The underlying cursor.
    pub cursor: C,
    /// Hashed address used for cursor positioning.
    hashed_address: B256,
}

impl<C> PackedDatabaseStorageTrieCursor<C> {
    /// Create a new packed storage trie cursor.
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<C> PackedDatabaseStorageTrieCursor<C>
where
    C: DbCursorRO<tables::StoragesTrieV2>
        + DbCursorRW<tables::StoragesTrieV2>
        + DbDupCursorRO<tables::StoragesTrieV2>
        + DbDupCursorRW<tables::StoragesTrieV2>,
{
    /// Writes storage updates that are already sorted.
    pub fn write_storage_trie_updates_sorted(
        &mut self,
        updates: &StorageTrieUpdatesSorted,
    ) -> Result<usize, DatabaseError> {
        if updates.is_deleted() && self.cursor.seek_exact(self.hashed_address)?.is_some() {
            self.cursor.delete_current_duplicates()?;
        }

        let mut num_entries = 0;
        for (nibbles, maybe_updated) in updates.storage_nodes.iter().filter(|(n, _)| !n.is_empty())
        {
            num_entries += 1;
            let nibbles = PackedStoredNibblesSubKey(*nibbles);
            if self
                .cursor
                .seek_by_key_subkey(self.hashed_address, nibbles.clone())?
                .filter(|e| e.nibbles == nibbles)
                .is_some()
            {
                self.cursor.delete_current()?;
            }

            if let Some(node) = maybe_updated {
                self.cursor.upsert(
                    self.hashed_address,
                    &PackedStorageTrieEntry { nibbles, node: node.clone() },
                )?;
            }
        }

        Ok(num_entries)
    }
}

impl<C> TrieCursor for PackedDatabaseStorageTrieCursor<C>
where
    C: DbCursorRO<tables::StoragesTrieV2> + DbDupCursorRO<tables::StoragesTrieV2> + Send,
{
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, PackedStoredNibblesSubKey(key))?
            .filter(|e| e.nibbles == PackedStoredNibblesSubKey(key))
            .map(|value| (value.nibbles.0, value.node)))
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, PackedStoredNibblesSubKey(key))?
            .map(|value| (value.nibbles.0, value.node)))
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.cursor.next_dup()?.map(|(_, v)| (v.nibbles.0, v.node)))
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.cursor.current()?.map(|(_, v)| v.nibbles.0))
    }

    fn reset(&mut self) {}
}

impl<C> TrieStorageCursor for PackedDatabaseStorageTrieCursor<C>
where
    C: DbCursorRO<tables::StoragesTrieV2> + DbDupCursorRO<tables::StoragesTrieV2> + Send,
{
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.hashed_address = hashed_address;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex_literal::hex;
    use reth_db_api::{cursor::DbCursorRW, transaction::DbTxMut};
    use reth_provider::test_utils::create_test_provider_factory;

    #[test]
    fn test_account_trie_order() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();

        let data = vec![
            hex!("0303040e").to_vec(),
            hex!("030305").to_vec(),
            hex!("03030500").to_vec(),
            hex!("0303050a").to_vec(),
        ];

        for key in data.clone() {
            cursor
                .upsert(
                    key.into(),
                    &BranchNodeCompact::new(
                        0b0000_0010_0000_0001,
                        0b0000_0010_0000_0001,
                        0,
                        Vec::default(),
                        None,
                    ),
                )
                .unwrap();
        }

        let db_data = cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(db_data[0].0 .0.to_vec(), data[0]);
        assert_eq!(db_data[1].0 .0.to_vec(), data[1]);
        assert_eq!(db_data[2].0 .0.to_vec(), data[2]);
        assert_eq!(db_data[3].0 .0.to_vec(), data[3]);

        assert_eq!(
            cursor.seek(hex!("0303040f").to_vec().into()).unwrap().map(|(k, _)| k.0.to_vec()),
            Some(data[1].clone())
        );
    }

    // tests that upsert and seek match on the storage trie cursor
    #[test]
    fn test_storage_cursor_abstraction() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let hashed_address = B256::random();
        let key = StoredNibblesSubKey::from(vec![0x2, 0x3]);
        let value = BranchNodeCompact::new(1, 1, 1, vec![B256::random()], None);

        cursor
            .upsert(hashed_address, &StorageTrieEntry { nibbles: key.clone(), node: value.clone() })
            .unwrap();

        let mut cursor = DatabaseStorageTrieCursor::new(cursor, hashed_address);
        assert_eq!(cursor.seek(key.into()).unwrap().unwrap().1, value);
    }
}
