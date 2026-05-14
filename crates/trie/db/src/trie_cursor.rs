use alloy_primitives::B256;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, Key, Table, Value},
    tables::{self, PackedAccountsTrie, PackedStoragesTrie},
    transaction::DbTx,
    DatabaseError,
};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::StorageTrieUpdatesSorted,
    BranchNodeCompact, Nibbles, PackedStorageTrieEntry, PackedStoredNibbles,
    PackedStoredNibblesSubKey, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey,
};
use std::marker::PhantomData;

/// Trait abstracting nibble encoding for trie keys.
///
/// Allows the same cursor implementation to work with both legacy (65-byte) and
/// packed (33-byte) nibble encodings. The underlying cursor types are monomorphized per
/// adapter, while [`DatabaseTrieCursorFactory`] selects the encoding at runtime.
pub trait TrieKeyAdapter: Clone + Send + Sync + 'static {
    /// The key type for account trie lookups (e.g., `StoredNibbles` or `PackedStoredNibbles`).
    type AccountKey: Key + From<Nibbles> + Clone;

    /// The subkey type for storage trie `DupSort` lookups
    /// (e.g., `StoredNibblesSubKey` or `PackedStoredNibblesSubKey`).
    type StorageSubKey: Key + From<Nibbles> + Clone + PartialEq;

    /// The storage trie entry type that pairs a subkey with a `BranchNodeCompact`.
    type StorageValue: Value + StorageTrieEntryLike<SubKey = Self::StorageSubKey>;

    /// Convert an account key back to `Nibbles`.
    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles;

    /// Convert a storage subkey back to `Nibbles`.
    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles;
}

/// Trait for storage trie entry types that carry a subkey and node.
///
/// Needed because [`StorageTrieEntry`] and [`PackedStorageTrieEntry`] are separate structs
/// with different field types, but `DatabaseStorageTrieCursor` must access `.nibbles()` and
/// `.node()` generically through `A::StorageValue`.
pub trait StorageTrieEntryLike: Sized {
    /// The subkey type.
    type SubKey: Clone;

    /// Returns a reference to the nibbles subkey.
    fn nibbles(&self) -> &Self::SubKey;

    /// Returns a reference to the branch node.
    fn node(&self) -> &BranchNodeCompact;

    /// Decompose this value into owned parts.
    fn into_parts(self) -> (Self::SubKey, BranchNodeCompact);

    /// Construct a new entry from a subkey and node.
    fn new(nibbles: Self::SubKey, node: BranchNodeCompact) -> Self;
}

impl StorageTrieEntryLike for StorageTrieEntry {
    type SubKey = StoredNibblesSubKey;

    fn nibbles(&self) -> &Self::SubKey {
        &self.nibbles
    }

    fn node(&self) -> &BranchNodeCompact {
        &self.node
    }

    fn into_parts(self) -> (Self::SubKey, BranchNodeCompact) {
        (self.nibbles, self.node)
    }

    fn new(nibbles: Self::SubKey, node: BranchNodeCompact) -> Self {
        Self { nibbles, node }
    }
}

/// Legacy (v1) nibble encoding: 1 nibble per byte, 65-byte subkeys.
#[derive(Debug, Clone)]
pub struct LegacyKeyAdapter;

impl TrieKeyAdapter for LegacyKeyAdapter {
    type AccountKey = StoredNibbles;
    type StorageSubKey = StoredNibblesSubKey;
    type StorageValue = StorageTrieEntry;

    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles {
        key.0
    }

    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles {
        subkey.0
    }
}

impl StorageTrieEntryLike for PackedStorageTrieEntry {
    type SubKey = PackedStoredNibblesSubKey;

    fn nibbles(&self) -> &Self::SubKey {
        &self.nibbles
    }

    fn node(&self) -> &BranchNodeCompact {
        &self.node
    }

    fn into_parts(self) -> (Self::SubKey, BranchNodeCompact) {
        (self.nibbles, self.node)
    }

    fn new(nibbles: Self::SubKey, node: BranchNodeCompact) -> Self {
        Self { nibbles, node }
    }
}

/// Packed (v2) nibble encoding: 2 nibbles per byte, 33-byte subkeys.
#[derive(Debug, Clone)]
pub struct PackedKeyAdapter;

impl TrieKeyAdapter for PackedKeyAdapter {
    type AccountKey = PackedStoredNibbles;
    type StorageSubKey = PackedStoredNibblesSubKey;
    type StorageValue = PackedStorageTrieEntry;

    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles {
        key.0
    }

    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles {
        subkey.0
    }
}

/// Helper trait to map a [`TrieKeyAdapter`] to the correct table types.
///
/// This indirection is needed because the `tables!` macro generates non-generic
/// table types, so we use separate "view" types for packed encoding that share
/// the same MDBX table name.
pub trait TrieTableAdapter: TrieKeyAdapter {
    /// The account trie table type.
    type AccountTrieTable: Table<Key = Self::AccountKey, Value = BranchNodeCompact>;
    /// The storage trie table type.
    type StorageTrieTable: Table<Key = B256, Value = Self::StorageValue>
        + DupSort<SubKey = Self::StorageSubKey>;
}

impl TrieTableAdapter for LegacyKeyAdapter {
    type AccountTrieTable = tables::AccountsTrie;
    type StorageTrieTable = tables::StoragesTrie;
}

impl TrieTableAdapter for PackedKeyAdapter {
    type AccountTrieTable = PackedAccountsTrie;
    type StorageTrieTable = PackedStoragesTrie;
}

/// Wrapper struct for database transaction implementing trie cursor factory trait.
#[derive(Debug, Clone)]
pub struct DatabaseTrieCursorFactory<T, A: TrieKeyAdapter> {
    tx: T,
    _adapter: PhantomData<A>,
}

impl<T, A: TrieKeyAdapter> DatabaseTrieCursorFactory<T, A> {
    /// Create new [`DatabaseTrieCursorFactory`].
    pub const fn new(tx: T) -> Self {
        Self { tx, _adapter: PhantomData }
    }
}

impl<TX, A> TrieCursorFactory for DatabaseTrieCursorFactory<&TX, A>
where
    TX: DbTx,
    A: TrieTableAdapter,
{
    type AccountTrieCursor<'a>
        = DatabaseAccountTrieCursor<<TX as DbTx>::Cursor<A::AccountTrieTable>, A>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = DatabaseStorageTrieCursor<<TX as DbTx>::DupCursor<A::StorageTrieTable>, A>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseAccountTrieCursor::new(self.tx.cursor_read::<A::AccountTrieTable>()?))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseStorageTrieCursor::new(
            self.tx.cursor_dup_read::<A::StorageTrieTable>()?,
            hashed_address,
        ))
    }
}

/// A cursor over the account trie.
#[derive(Debug)]
pub struct DatabaseAccountTrieCursor<C, A: TrieKeyAdapter>(C, PhantomData<A>);

impl<C, A: TrieKeyAdapter> DatabaseAccountTrieCursor<C, A> {
    /// Create a new account trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor, PhantomData)
    }
}

impl<C, A> TrieCursor for DatabaseAccountTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::AccountTrieTable> + Send,
{
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .0
            .seek_exact(A::AccountKey::from(key))?
            .map(|value| (A::account_key_to_nibbles(&value.0), value.1)))
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .0
            .seek(A::AccountKey::from(key))?
            .map(|value| (A::account_key_to_nibbles(&value.0), value.1)))
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.next()?.map(|value| (A::account_key_to_nibbles(&value.0), value.1)))
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.0.current()?.map(|(k, _)| A::account_key_to_nibbles(&k)))
    }

    fn reset(&mut self) {
        // No-op for database cursors
    }
}

/// A cursor over the storage tries stored in the database.
#[derive(Debug)]
pub struct DatabaseStorageTrieCursor<C, A: TrieKeyAdapter> {
    /// The underlying cursor.
    pub cursor: C,
    /// Hashed address used for cursor positioning.
    hashed_address: B256,
    _adapter: PhantomData<A>,
}

impl<C, A: TrieKeyAdapter> DatabaseStorageTrieCursor<C, A> {
    /// Create a new storage trie cursor.
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address, _adapter: PhantomData }
    }
}

impl<C, A> DatabaseStorageTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable>
        + DbCursorRW<A::StorageTrieTable>
        + DbDupCursorRO<A::StorageTrieTable>
        + DbDupCursorRW<A::StorageTrieTable>,
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
            let nibbles = A::StorageSubKey::from(*nibbles);
            // Delete the old entry if it exists.
            if self
                .cursor
                .seek_by_key_subkey(self.hashed_address, nibbles.clone())?
                .as_ref()
                .is_some_and(|e| *e.nibbles() == nibbles)
            {
                self.cursor.delete_current()?;
            }

            // There is an updated version of this node, insert new entry.
            if let Some(node) = maybe_updated {
                self.cursor
                    .upsert(self.hashed_address, &A::StorageValue::new(nibbles, node.clone()))?;
            }
        }

        Ok(num_entries)
    }
}

impl<C, A> TrieCursor for DatabaseStorageTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable> + DbDupCursorRO<A::StorageTrieTable> + Send,
{
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let subkey = A::StorageSubKey::from(key);
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, subkey.clone())?
            .filter(|e| *e.nibbles() == subkey)
            .map(|value| {
                let (subkey, node) = value.into_parts();
                (A::subkey_to_nibbles(&subkey), node)
            }))
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.cursor.seek_by_key_subkey(self.hashed_address, A::StorageSubKey::from(key))?.map(
            |value| {
                let (subkey, node) = value.into_parts();
                (A::subkey_to_nibbles(&subkey), node)
            },
        ))
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.cursor.next_dup()?.map(|(_, value)| {
            let (subkey, node) = value.into_parts();
            (A::subkey_to_nibbles(&subkey), node)
        }))
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.cursor.current()?.map(|(_, v)| A::subkey_to_nibbles(v.nibbles())))
    }

    fn reset(&mut self) {
        // No-op for database cursors
    }
}

impl<C, A> TrieStorageCursor for DatabaseStorageTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable> + DbDupCursorRO<A::StorageTrieTable> + Send,
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
        use reth_storage_api::StorageSettingsCache;
        use reth_trie::trie_cursor::{TrieCursor, TrieCursorFactory};

        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let hashed_address = B256::random();
        let key = StoredNibblesSubKey::from(vec![0x2, 0x3]);
        let value = BranchNodeCompact::new(1, 1, 1, vec![B256::random()], None);

        cursor
            .upsert(hashed_address, &StorageTrieEntry { nibbles: key.clone(), node: value.clone() })
            .unwrap();

        crate::with_adapter!(provider, |A| {
            let trie_factory = DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref());
            let mut cursor = trie_factory.storage_trie_cursor(hashed_address).unwrap();
            assert_eq!(cursor.seek(key.into()).unwrap().unwrap().1, value);
        });
    }
}
