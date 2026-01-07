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
    C: DbCursorRO<tables::AccountsTrie> + Send + Sync,
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
///
/// This cursor implements locality optimization: when seeking forward from the current position,
/// it uses `next_dup` (O(1)) to walk forward instead of `seek_by_key_subkey` (O(log N)) when
/// the target key is close to the current position.
#[derive(Debug)]
pub struct DatabaseStorageTrieCursor<C> {
    /// The underlying cursor.
    pub cursor: C,
    /// Hashed address used for cursor positioning.
    hashed_address: B256,
    /// The last key returned by the cursor, used for locality optimization.
    /// When seeking forward from this position, we can use `next_dup` instead of re-seeking.
    last_key: Option<Nibbles>,
}

impl<C> DatabaseStorageTrieCursor<C> {
    /// Create a new storage trie cursor.
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address, last_key: None }
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
        // Invalidate cursor position tracking: writes move the cursor arbitrarily
        self.last_key = None;

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
    C: DbCursorRO<tables::StoragesTrie> + DbDupCursorRO<tables::StoragesTrie> + Send + Sync,
{
    /// Seeks an exact match for the given key in the storage trie.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let result = self
            .cursor
            .seek_by_key_subkey(self.hashed_address, StoredNibblesSubKey(key))?
            .filter(|e| e.nibbles == StoredNibblesSubKey(key))
            .map(|value| (value.nibbles.0, value.node));

        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    /// Seeks the given key in the storage trie.
    ///
    /// This method implements a bounded locality optimization: if the cursor is already positioned
    /// and the target key is > the current position, we speculatively walk forward using
    /// `next_dup` (O(1) per step) for up to `MAX_WALK_STEPS` entries. If the target is not
    /// found within that limit, we fall back to `seek_by_key_subkey` (O(log N)).
    ///
    /// This bounds the worst-case complexity to O(MAX_WALK_STEPS + log N) instead of O(N).
    #[allow(clippy::collapsible_if)]
    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        // Maximum forward walk steps before falling back to seek.
        // This bounds worst-case from O(N) to O(MAX_WALK_STEPS + log N).
        // Value of 8 is optimal for tries up to ~256 entries (log2(256) = 8).
        const MAX_WALK_STEPS: usize = 8;

        // Locality optimization: if cursor is positioned and target is ahead,
        // speculatively walk forward for a bounded number of steps
        if let Some(last) = self.last_key {
            if key > last {
                // Bounded forward walk - only beneficial for nearby keys
                for _ in 0..MAX_WALK_STEPS {
                    match self.cursor.next_dup()? {
                        Some((_, entry)) => {
                            if entry.nibbles.0 >= key {
                                // Found target or passed it
                                self.last_key = Some(entry.nibbles.0);
                                return Ok(Some((entry.nibbles.0, entry.node)));
                            }
                            // Haven't reached target yet, continue walking
                        }
                        None => {
                            // Exhausted all duplicates without finding target
                            self.last_key = None;
                            return Ok(None);
                        }
                    }
                }
                // Exceeded walk limit - fall through to seek_by_key_subkey
            } else if key == last {
                // Re-seeking the same key, return current position if still valid
                if let Some((_, entry)) = self.cursor.current()? {
                    if entry.nibbles.0 == key {
                        return Ok(Some((entry.nibbles.0, entry.node)));
                    }
                }
            }
        }

        // Fall back to seek_by_key_subkey for:
        // - Backward seeks (key < last)
        // - When cursor is not positioned (last_key is None)
        // - When forward walk exceeded limit
        let result = self
            .cursor
            .seek_by_key_subkey(self.hashed_address, StoredNibblesSubKey(key))?
            .map(|value| (value.nibbles.0, value.node));

        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let result = self.cursor.next_dup()?.map(|(_, v)| (v.nibbles.0, v.node));
        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    /// Retrieves the current value in the storage trie cursor.
    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.cursor.current()?.map(|(_, v)| v.nibbles.0))
    }

    fn reset(&mut self) {
        self.last_key = None;
    }
}

impl<C> TrieStorageCursor for DatabaseStorageTrieCursor<C>
where
    C: DbCursorRO<tables::StoragesTrie> + DbDupCursorRO<tables::StoragesTrie> + Send + Sync,
{
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.hashed_address = hashed_address;
        // Reset cursor position tracking when switching to a different storage trie
        self.last_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex_literal::hex;
    use reth_db_api::{cursor::DbCursorRW, transaction::DbTxMut};
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_trie::trie_cursor::TrieStorageCursor;

    fn create_test_nibbles(count: usize) -> Vec<Nibbles> {
        (0..count as u64)
            .map(|i| {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&i.to_be_bytes());
                Nibbles::unpack(bytes)
            })
            .collect()
    }

    fn create_test_node() -> BranchNodeCompact {
        BranchNodeCompact::new(0b1111, 0b0011, 0, vec![], None)
    }

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
    fn test_storage_trie_forward_sequential_seek() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_nibbles(10);

        // Insert test data
        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for key in &keys {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: create_test_node(),
                        },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseStorageTrieCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap(),
            hashed_address,
        );

        // Forward sequential seeks should all succeed
        for (i, key) in keys.iter().enumerate() {
            let result = cursor.seek(*key).unwrap();
            assert!(result.is_some(), "Should find key at index {i}");
            let (found_key, _) = result.unwrap();
            assert_eq!(found_key, *key);
        }
    }

    #[test]
    fn test_storage_trie_backward_seek_fallback() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_nibbles(10);

        // Insert test data
        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for key in &keys {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: create_test_node(),
                        },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseStorageTrieCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap(),
            hashed_address,
        );

        // Seek to last key first
        let result = cursor.seek(keys[9]).unwrap();
        assert!(result.is_some());

        // Backward seek should still work
        let result = cursor.seek(keys[2]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, keys[2]);
    }

    #[test]
    fn test_storage_trie_cursor_exhaustion() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_nibbles(5);

        // Insert test data
        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for key in &keys {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: create_test_node(),
                        },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseStorageTrieCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap(),
            hashed_address,
        );

        // Position cursor
        let _ = cursor.seek(keys[0]).unwrap();

        // Create a very high nibbles key to exhaust
        let high_key = Nibbles::from_nibbles([0xf; 64]);

        // Seek past all entries
        let result = cursor.seek(high_key).unwrap();
        assert!(result.is_none());

        // Now seek back - should work via fallback
        let result = cursor.seek(keys[0]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, keys[0]);
    }

    #[test]
    fn test_storage_trie_address_switch() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let address1 = B256::random();
        let address2 = B256::random();
        let keys = create_test_nibbles(5);

        let node1 = BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None);
        let node2 = BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None);

        // Insert test data for both addresses
        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for key in &keys {
                cursor
                    .upsert(
                        address1,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: node1.clone(),
                        },
                    )
                    .unwrap();
                cursor
                    .upsert(
                        address2,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: node2.clone(),
                        },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseStorageTrieCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap(),
            address1,
        );

        // Seek in first address
        let result = cursor.seek(keys[2]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().1, node1);

        // Switch address and seek
        cursor.set_hashed_address(address2);
        let result = cursor.seek(keys[2]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().1, node2);
    }

    #[test]
    fn test_storage_trie_seek_same_key() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_nibbles(5);

        // Insert test data
        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for key in &keys {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: create_test_node(),
                        },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseStorageTrieCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap(),
            hashed_address,
        );

        // Seek to a key
        let result1 = cursor.seek(keys[2]).unwrap();
        assert!(result1.is_some());

        // Seek to the same key again
        let result2 = cursor.seek(keys[2]).unwrap();
        assert!(result2.is_some());
        assert_eq!(result1.unwrap().0, result2.unwrap().0);
    }

    #[test]
    fn test_storage_trie_reset() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_nibbles(5);

        // Insert test data
        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for key in &keys {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: create_test_node(),
                        },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseStorageTrieCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap(),
            hashed_address,
        );

        // Position cursor
        let _ = cursor.seek(keys[2]).unwrap();

        // Reset and verify we can still seek
        cursor.reset();
        let result = cursor.seek(keys[0]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, keys[0]);
    }

    #[test]
    fn test_storage_trie_seek_exact_updates_last_key() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_nibbles(5);

        // Insert test data
        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            for key in &keys {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageTrieEntry {
                            nibbles: StoredNibblesSubKey(*key),
                            node: create_test_node(),
                        },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseStorageTrieCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap(),
            hashed_address,
        );

        // seek_exact should update last_key
        let result = cursor.seek_exact(keys[2]).unwrap();
        assert!(result.is_some());

        // Forward seek should use optimization
        let result = cursor.seek(keys[3]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, keys[3]);
    }
}
