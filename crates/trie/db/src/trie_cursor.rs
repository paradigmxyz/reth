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
    BranchNodeCompact, Nibbles, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey,
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
    /// Writes storage updates that are already sorted.
    ///
    /// Uses a single linear cursor scan to delete old entries rather than seeking
    /// from scratch for each update. Since both the existing `DupSort` entries and the
    /// updates are sorted by nibbles, we merge-scan them in `O(existing + updates)` cursor
    /// moves instead of `O(updates × tree_depth)` seeks.
    pub fn write_storage_trie_updates_sorted(
        &mut self,
        updates: &StorageTrieUpdatesSorted,
    ) -> Result<usize, DatabaseError> {
        // The storage trie for this account has to be deleted.
        if updates.is_deleted() && self.cursor.seek_exact(self.hashed_address)?.is_some() {
            self.cursor.delete_current_duplicates()?;
        }

        let non_empty_updates: Vec<_> =
            updates.storage_nodes.iter().filter(|(n, _)| !n.is_empty()).collect();

        if non_empty_updates.is_empty() {
            return Ok(0);
        }

        // Phase 1: Linear merge-scan to delete existing entries that will be
        // updated or removed. We seek once to the first update's nibbles, then
        // advance via next_dup — avoiding a full B-tree seek per update.
        let first_target = StoredNibblesSubKey(non_empty_updates[0].0);
        let mut db_entry = self.cursor.seek_by_key_subkey(self.hashed_address, first_target)?;

        for &(nibbles, _) in &non_empty_updates {
            let target = StoredNibblesSubKey(*nibbles);

            // Advance cursor past existing entries that sort before this update
            loop {
                match &db_entry {
                    Some(entry) if entry.nibbles < target => {
                        db_entry = self.cursor.next_dup()?.map(|(_, v)| v);
                    }
                    _ => break,
                }
            }

            if db_entry.as_ref().is_some_and(|e| e.nibbles == target) {
                self.cursor.delete_current()?;
                // MDBX positions the cursor at the successor after delete.
                // Use current() to read it without advancing further. Verify
                // the key still matches our hashed_address since the cursor
                // may have moved to the next primary key.
                db_entry = self
                    .cursor
                    .current()?
                    .filter(|(k, _)| *k == self.hashed_address)
                    .map(|(_, v)| v);
            }
        }

        // Phase 2: Insert updated entries.
        let mut num_entries = 0;
        for &(nibbles, maybe_updated) in &non_empty_updates {
            num_entries += 1;
            if let Some(node) = maybe_updated {
                self.cursor.upsert(
                    self.hashed_address,
                    &StorageTrieEntry {
                        nibbles: StoredNibblesSubKey(*nibbles),
                        node: node.clone(),
                    },
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

    #[test]
    fn test_write_storage_trie_updates_sorted_batch() {
        use reth_trie::updates::StorageTrieUpdatesSorted;

        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();

        // Insert some existing entries
        let nibbles_a = Nibbles::from_nibbles_unchecked(vec![0x01, 0x02]);
        let nibbles_b = Nibbles::from_nibbles_unchecked(vec![0x03, 0x04]);
        let nibbles_c = Nibbles::from_nibbles_unchecked(vec![0x05, 0x06]);
        let nibbles_d = Nibbles::from_nibbles_unchecked(vec![0x07, 0x08]);

        let node_v1 = BranchNodeCompact::new(0b0001, 0b0001, 0, Vec::new(), None);
        let node_v2 = BranchNodeCompact::new(0b0010, 0b0010, 0, Vec::new(), None);

        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for nibbles in [&nibbles_a, &nibbles_b, &nibbles_c, &nibbles_d] {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(nibbles.clone()),
                            node: node_v1.clone(),
                        },
                    )
                    .unwrap();
            }
        }

        // Create sorted updates:
        // - nibbles_a: update to node_v2
        // - nibbles_b: delete (None)
        // - nibbles_c: unchanged (not in updates)
        // - nibbles_d: update to node_v2
        // - nibbles_e: new entry
        let nibbles_e = Nibbles::from_nibbles_unchecked(vec![0x09, 0x0a]);
        let updates = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (nibbles_a.clone(), Some(node_v2.clone())),
                (nibbles_b.clone(), None),
                (nibbles_d.clone(), Some(node_v2.clone())),
                (nibbles_e.clone(), Some(node_v2.clone())),
            ],
        };

        {
            let cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            let mut trie_cursor = DatabaseStorageTrieCursor::new(cursor, hashed_address);
            let num = trie_cursor.write_storage_trie_updates_sorted(&updates).unwrap();
            assert_eq!(num, 4);
        }

        // Verify final state
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap();

        // nibbles_a: updated to node_v2
        let entry = cursor
            .seek_by_key_subkey(hashed_address, StoredNibblesSubKey(nibbles_a.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(entry.nibbles, StoredNibblesSubKey(nibbles_a));
        assert_eq!(entry.node, node_v2);

        // nibbles_b: deleted
        let entry = cursor
            .seek_by_key_subkey(hashed_address, StoredNibblesSubKey(nibbles_b.clone()))
            .unwrap();
        assert!(entry.is_none() || entry.unwrap().nibbles != StoredNibblesSubKey(nibbles_b));

        // nibbles_c: unchanged (still node_v1)
        let entry = cursor
            .seek_by_key_subkey(hashed_address, StoredNibblesSubKey(nibbles_c.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(entry.nibbles, StoredNibblesSubKey(nibbles_c));
        assert_eq!(entry.node, node_v1);

        // nibbles_d: updated to node_v2
        let entry = cursor
            .seek_by_key_subkey(hashed_address, StoredNibblesSubKey(nibbles_d.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(entry.nibbles, StoredNibblesSubKey(nibbles_d));
        assert_eq!(entry.node, node_v2);

        // nibbles_e: new entry with node_v2
        let entry = cursor
            .seek_by_key_subkey(hashed_address, StoredNibblesSubKey(nibbles_e.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(entry.nibbles, StoredNibblesSubKey(nibbles_e));
        assert_eq!(entry.node, node_v2);
    }

    #[test]
    fn test_write_storage_trie_updates_consecutive_deletes() {
        use reth_trie::updates::StorageTrieUpdatesSorted;

        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();

        let node = BranchNodeCompact::new(0b0001, 0b0001, 0, Vec::new(), None);

        let nibbles: Vec<_> =
            (0..6u8).map(|i| Nibbles::from_nibbles_unchecked(vec![i, i + 1])).collect();

        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for n in &nibbles {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(n.clone()),
                            node: node.clone(),
                        },
                    )
                    .unwrap();
            }
        }

        // Delete consecutive entries [1], [2], [3] while keeping [0], [4], [5]
        let updates = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (nibbles[1].clone(), None),
                (nibbles[2].clone(), None),
                (nibbles[3].clone(), None),
            ],
        };

        {
            let cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            let mut trie_cursor = DatabaseStorageTrieCursor::new(cursor, hashed_address);
            trie_cursor.write_storage_trie_updates_sorted(&updates).unwrap();
        }

        // Verify: [0], [4], [5] remain; [1], [2], [3] are gone
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap();

        let entry = cursor
            .seek_by_key_subkey(hashed_address, StoredNibblesSubKey(nibbles[0].clone()))
            .unwrap()
            .unwrap();
        assert_eq!(entry.nibbles, StoredNibblesSubKey(nibbles[0].clone()));

        // After [0], next dup should be [4] (not [1], [2], or [3])
        let entry = cursor.next_dup().unwrap().unwrap();
        assert_eq!(entry.1.nibbles, StoredNibblesSubKey(nibbles[4].clone()));

        let entry = cursor.next_dup().unwrap().unwrap();
        assert_eq!(entry.1.nibbles, StoredNibblesSubKey(nibbles[5].clone()));

        // No more dups
        assert!(cursor.next_dup().unwrap().is_none());
    }
}
