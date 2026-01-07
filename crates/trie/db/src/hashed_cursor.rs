use alloy_primitives::{B256, U256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_primitives_traits::Account;
use reth_trie::hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

/// A struct wrapping database transaction that implements [`HashedCursorFactory`].
#[derive(Debug, Clone)]
pub struct DatabaseHashedCursorFactory<T>(T);

impl<T> DatabaseHashedCursorFactory<T> {
    /// Create new database hashed cursor factory.
    pub const fn new(tx: T) -> Self {
        Self(tx)
    }
}

impl<TX: DbTx> HashedCursorFactory for DatabaseHashedCursorFactory<&TX> {
    type AccountCursor<'a>
        = DatabaseHashedAccountCursor<<TX as DbTx>::Cursor<tables::HashedAccounts>>
    where
        Self: 'a;
    type StorageCursor<'a>
        = DatabaseHashedStorageCursor<<TX as DbTx>::DupCursor<tables::HashedStorages>>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(DatabaseHashedAccountCursor(self.0.cursor_read::<tables::HashedAccounts>()?))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(DatabaseHashedStorageCursor::new(
            self.0.cursor_dup_read::<tables::HashedStorages>()?,
            hashed_address,
        ))
    }
}

/// A struct wrapping database cursor over hashed accounts implementing [`HashedCursor`] for
/// iterating over accounts.
#[derive(Debug)]
pub struct DatabaseHashedAccountCursor<C>(C);

impl<C> DatabaseHashedAccountCursor<C> {
    /// Create new database hashed account cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> HashedCursor for DatabaseHashedAccountCursor<C>
where
    C: DbCursorRO<tables::HashedAccounts>,
{
    type Value = Account;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.next()
    }

    fn reset(&mut self) {
        // Database cursors are stateless, no reset needed
    }
}

/// The structure wrapping a database cursor for hashed storage and
/// a target hashed address. Implements [`HashedCursor`] and [`HashedStorageCursor`]
/// for iterating over hashed storage.
///
/// This cursor implements locality optimization: when seeking forward from the current position,
/// it uses `next_dup_val` (O(1)) to walk forward instead of `seek_by_key_subkey` (O(log N)) when
/// the target key is close to the current position.
#[derive(Debug)]
pub struct DatabaseHashedStorageCursor<C> {
    /// Database hashed storage cursor.
    cursor: C,
    /// Target hashed address of the account that the storage belongs to.
    hashed_address: B256,
    /// The last key returned by the cursor, used for locality optimization.
    /// When seeking forward from this position, we can use `next_dup_val` instead of re-seeking.
    last_key: Option<B256>,
}

impl<C> DatabaseHashedStorageCursor<C> {
    /// Create new [`DatabaseHashedStorageCursor`].
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address, last_key: None }
    }
}

impl<C> HashedCursor for DatabaseHashedStorageCursor<C>
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
{
    type Value = U256;

    /// Seeks the given key in the hashed storage.
    ///
    /// This method implements a bounded locality optimization: if the cursor is already positioned
    /// and the target key is > the current position, we speculatively walk forward using
    /// `next_dup_val` (O(1) per step) for up to `MAX_WALK_STEPS` entries. If the target is not
    /// found within that limit, we fall back to `seek_by_key_subkey` (O(log N)).
    ///
    /// This bounds the worst-case complexity to O(MAX_WALK_STEPS + log N) instead of O(N).
    #[allow(clippy::collapsible_if)]
    fn seek(&mut self, subkey: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        // Maximum forward walk steps before falling back to seek.
        // This bounds worst-case from O(N) to O(MAX_WALK_STEPS + log N).
        // Value of 8 is optimal for tries up to ~256 entries (log2(256) = 8).
        const MAX_WALK_STEPS: usize = 8;

        // Locality optimization: if cursor is positioned and target is ahead,
        // speculatively walk forward for a bounded number of steps
        if let Some(last) = self.last_key {
            if subkey > last {
                // Bounded forward walk - only beneficial for nearby keys
                for _ in 0..MAX_WALK_STEPS {
                    match self.cursor.next_dup_val()? {
                        Some(entry) => {
                            if entry.key >= subkey {
                                // Found target or passed it
                                self.last_key = Some(entry.key);
                                return Ok(Some((entry.key, entry.value)));
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
            } else if subkey == last {
                // Re-seeking the same key, return current position if still valid
                if let Some((_, entry)) = self.cursor.current()? {
                    if entry.key == subkey {
                        return Ok(Some((entry.key, entry.value)));
                    }
                }
            }
        }

        // Fall back to seek_by_key_subkey for:
        // - Backward seeks (subkey < last)
        // - When cursor is not positioned (last_key is None)
        // - When forward walk exceeded limit
        let result =
            self.cursor.seek_by_key_subkey(self.hashed_address, subkey)?.map(|e| (e.key, e.value));
        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let result = self.cursor.next_dup_val()?.map(|e| (e.key, e.value));
        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    fn reset(&mut self) {
        self.last_key = None;
    }
}

impl<C> HashedStorageCursor for DatabaseHashedStorageCursor<C>
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        Ok(self.cursor.seek_exact(self.hashed_address)?.is_none())
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.hashed_address = hashed_address;
        // Reset cursor position tracking when switching to a different storage
        self.last_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db_api::{cursor::DbCursorRW, transaction::DbTxMut};
    use reth_primitives_traits::StorageEntry;
    use reth_provider::test_utils::create_test_provider_factory;

    fn create_test_keys(count: usize) -> Vec<B256> {
        (0..count as u64)
            .map(|i| {
                let mut bytes = [0u8; 32];
                bytes[24..32].copy_from_slice(&i.to_be_bytes());
                B256::from(bytes)
            })
            .collect()
    }

    #[test]
    fn test_forward_sequential_seek_uses_optimization() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_keys(10);

        // Insert test data
        {
            let mut cursor =
                provider.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
            for (i, key) in keys.iter().enumerate() {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageEntry { key: *key, value: U256::from(i as u64) },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseHashedStorageCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap(),
            hashed_address,
        );

        // Forward sequential seeks should all succeed
        for (i, key) in keys.iter().enumerate() {
            let result = cursor.seek(*key).unwrap();
            assert!(result.is_some(), "Should find key at index {i}");
            let (found_key, value) = result.unwrap();
            assert_eq!(found_key, *key);
            assert_eq!(value, U256::from(i as u64));
        }
    }

    #[test]
    fn test_backward_seek_falls_back_to_seek_by_key_subkey() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_keys(10);

        // Insert test data
        {
            let mut cursor =
                provider.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
            for (i, key) in keys.iter().enumerate() {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageEntry { key: *key, value: U256::from(i as u64) },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseHashedStorageCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap(),
            hashed_address,
        );

        // Seek to last key first
        let result = cursor.seek(keys[9]).unwrap();
        assert!(result.is_some());

        // Backward seek should still work (falls back to seek_by_key_subkey)
        let result = cursor.seek(keys[2]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, keys[2]);
    }

    #[test]
    fn test_cursor_exhaustion_then_new_seek() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_keys(5);

        // Insert test data
        {
            let mut cursor =
                provider.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
            for (i, key) in keys.iter().enumerate() {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageEntry { key: *key, value: U256::from(i as u64) },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseHashedStorageCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap(),
            hashed_address,
        );

        // Exhaust cursor by seeking past all keys
        let high_key = {
            let mut bytes = [0xffu8; 32];
            bytes[0] = 0xff;
            B256::from(bytes)
        };

        // First seek to position the cursor
        let _ = cursor.seek(keys[0]).unwrap();
        // Then seek past all entries
        let result = cursor.seek(high_key).unwrap();
        assert!(result.is_none(), "Should not find key past all entries");

        // Now seek back to an existing key - should work via fallback
        let result = cursor.seek(keys[0]).unwrap();
        assert!(result.is_some(), "Should find first key after exhaustion");
        assert_eq!(result.unwrap().0, keys[0]);
    }

    #[test]
    fn test_address_switch_resets_position() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let address1 = B256::random();
        let address2 = B256::random();
        let keys = create_test_keys(5);

        // Insert test data for both addresses
        {
            let mut cursor =
                provider.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
            for (i, key) in keys.iter().enumerate() {
                cursor
                    .upsert(address1, &StorageEntry { key: *key, value: U256::from(i as u64) })
                    .unwrap();
                cursor
                    .upsert(
                        address2,
                        &StorageEntry { key: *key, value: U256::from((i + 100) as u64) },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseHashedStorageCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap(),
            address1,
        );

        // Seek in first address
        let result = cursor.seek(keys[2]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().1, U256::from(2));

        // Switch address and seek
        cursor.set_hashed_address(address2);
        let result = cursor.seek(keys[2]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().1, U256::from(102));
    }

    #[test]
    fn test_seek_same_key_returns_current() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_keys(5);

        // Insert test data
        {
            let mut cursor =
                provider.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
            for (i, key) in keys.iter().enumerate() {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageEntry { key: *key, value: U256::from(i as u64) },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseHashedStorageCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap(),
            hashed_address,
        );

        // Seek to a key
        let result1 = cursor.seek(keys[2]).unwrap();
        assert!(result1.is_some());

        // Seek to the same key again - should use cached current position
        let result2 = cursor.seek(keys[2]).unwrap();
        assert!(result2.is_some());
        assert_eq!(result1.unwrap(), result2.unwrap());
    }

    #[test]
    fn test_reset_clears_last_key() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::random();
        let keys = create_test_keys(5);

        // Insert test data
        {
            let mut cursor =
                provider.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
            for (i, key) in keys.iter().enumerate() {
                cursor
                    .upsert(
                        hashed_address,
                        &StorageEntry { key: *key, value: U256::from(i as u64) },
                    )
                    .unwrap();
            }
        }

        let mut cursor = DatabaseHashedStorageCursor::new(
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap(),
            hashed_address,
        );

        // Seek to position cursor
        let _ = cursor.seek(keys[2]).unwrap();

        // Reset and verify we can still seek
        cursor.reset();
        let result = cursor.seek(keys[0]).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, keys[0]);
    }
}
