use alloy_primitives::B256;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, Key, Table, Value},
    tables::{
        self, MemcmpAccountsTrie, MemcmpStoragesTrie, PackedAccountsTrie, PackedStoragesTrie,
    },
    transaction::DbTx,
    DatabaseError,
};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::StorageTrieUpdatesSorted,
    BranchNodeCompact, MemcmpStorageTrieEntry, MemcmpStoredNibbles, MemcmpStoredNibblesSubKey,
    Nibbles, PackedStorageTrieEntry, PackedStoredNibbles, PackedStoredNibblesSubKey,
    StorageTrieEntry, StoredNibbles, StoredNibblesSubKey,
};
use std::marker::PhantomData;

/// Trait abstracting nibble encoding for trie keys.
///
/// Allows the same cursor implementation to work with legacy (65-byte), packed (33-byte) and
/// memcmp (5 to 40-byte) nibble encodings. [`DatabaseTrieCursorFactory`] fixes the encoding
/// through its adapter type parameter. Callers must choose an adapter matching the database's
/// storage settings; [`crate::with_adapter!`] provides runtime dispatch between the legacy and
/// packed adapters. No storage setting selects [`MemcmpKeyAdapter`] yet.
pub trait TrieKeyAdapter: Clone + Send + Sync + 'static {
    /// The key type for account trie lookups (e.g., `StoredNibbles`, `PackedStoredNibbles` or
    /// `MemcmpStoredNibbles`).
    type AccountKey: Key + From<Nibbles>;

    /// The subkey type for storage trie `DupSort` lookups
    /// (e.g., `StoredNibblesSubKey`, `PackedStoredNibblesSubKey` or `MemcmpStoredNibblesSubKey`).
    type StorageSubKey: Key + From<Nibbles>;

    /// The storage trie entry type that pairs a subkey with a `BranchNodeCompact`.
    type StorageValue: Value + StorageTrieEntryLike<SubKey = Self::StorageSubKey>;

    /// Convert an account key back to `Nibbles`.
    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles;

    /// Convert a storage subkey back to `Nibbles`.
    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles;
}

/// Trait for storage trie entry types that carry a subkey and node.
///
/// Provides access to the encoded subkey and branch node, as well as construction and
/// decomposition into owned parts, independently of the subkey encoding.
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

impl StorageTrieEntryLike for MemcmpStorageTrieEntry {
    type SubKey = MemcmpStoredNibblesSubKey;

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

/// Memcmp group nibble encoding: 8 nibbles per 5-byte group, 5 to 40-byte keys and subkeys.
///
/// Research adapter for a variable-length trie key encoding. It is not selected by any storage
/// setting, so it must only be used on trie tables written through it.
#[derive(Debug, Clone)]
pub struct MemcmpKeyAdapter;

impl TrieKeyAdapter for MemcmpKeyAdapter {
    type AccountKey = MemcmpStoredNibbles;
    type StorageSubKey = MemcmpStoredNibblesSubKey;
    type StorageValue = MemcmpStorageTrieEntry;

    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles {
        key.0
    }

    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles {
        subkey.0
    }
}

/// Maps a [`TrieKeyAdapter`] to account and storage trie tables using its key and value types.
///
/// The selected table types must match the database's stored nibble encoding.
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

impl TrieTableAdapter for MemcmpKeyAdapter {
    type AccountTrieTable = MemcmpAccountsTrie;
    type StorageTrieTable = MemcmpStoragesTrie;
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
    use std::collections::BTreeMap;

    fn account_trie_order<A: TrieTableAdapter>() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_write::<A::AccountTrieTable>().unwrap();

        // The 8 and 9 nibble keys end in zero nibbles, which the memcmp encoding pads with, and
        // cross its 8 nibble group boundary.
        let data = vec![
            hex!("0303040e").to_vec(),
            hex!("030305").to_vec(),
            hex!("03030500").to_vec(),
            hex!("0303050000000000").to_vec(),
            hex!("030305000000000000").to_vec(),
            hex!("03030500000000000a").to_vec(),
            hex!("0303050a").to_vec(),
        ];

        for key in data.clone() {
            cursor
                .upsert(
                    Nibbles::from_nibbles_unchecked(key).into(),
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

        let db_data = cursor
            .walk_range(..)
            .unwrap()
            .map(|entry| entry.map(|(key, _)| A::account_key_to_nibbles(&key).to_vec()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(db_data, data);

        assert_eq!(
            cursor
                .seek(Nibbles::from_nibbles_unchecked(hex!("0303040f")).into())
                .unwrap()
                .map(|(key, _)| A::account_key_to_nibbles(&key).to_vec()),
            Some(data[1].clone())
        );
    }

    #[test]
    fn test_account_trie_order_legacy() {
        account_trie_order::<LegacyKeyAdapter>();
    }

    #[test]
    fn test_account_trie_order_packed() {
        account_trie_order::<PackedKeyAdapter>();
    }

    #[test]
    fn test_account_trie_order_memcmp() {
        account_trie_order::<MemcmpKeyAdapter>();
    }

    // tests that upsert and seek match on the storage trie cursor.
    fn storage_cursor_abstraction<A: TrieTableAdapter>() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_write::<A::StorageTrieTable>().unwrap();

        let hashed_address = B256::random();
        let key = Nibbles::from_nibbles([0x2, 0x3]);
        let value = BranchNodeCompact::new(1, 1, 1, vec![B256::random()], None);

        cursor.upsert(hashed_address, &A::StorageValue::new(key.into(), value.clone())).unwrap();

        let trie_factory = DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref());
        let mut cursor = trie_factory.storage_trie_cursor(hashed_address).unwrap();
        assert_eq!(cursor.seek(key).unwrap().unwrap().1, value);
    }

    #[test]
    fn test_storage_cursor_abstraction_legacy() {
        storage_cursor_abstraction::<LegacyKeyAdapter>();
    }

    #[test]
    fn test_storage_cursor_abstraction_packed() {
        storage_cursor_abstraction::<PackedKeyAdapter>();
    }

    #[test]
    fn test_storage_cursor_abstraction_memcmp() {
        storage_cursor_abstraction::<MemcmpKeyAdapter>();
    }

    // tests that storage trie updates overwrite, delete and insert nodes of each address.
    fn storage_trie_write_path<A: TrieTableAdapter>() {
        // A node with `hashes` hashes, so overwrites change the value size.
        let node = |hashes: u16| {
            BranchNodeCompact::new(
                u16::MAX,
                0,
                u16::MAX >> (16 - hashes),
                vec![B256::repeat_byte(hashes as u8); hashes as usize],
                None,
            )
        };
        let path = |nibbles: &[u8]| Nibbles::from_nibbles_unchecked(nibbles);
        let zeros = |len: usize| path(&vec![0; len]);
        let updates = |mut nodes: Vec<(Nibbles, Option<BranchNodeCompact>)>| {
            nodes.sort_unstable_by_key(|(path, _)| *path);
            StorageTrieUpdatesSorted { storage_nodes: nodes }
        };

        // Adjacent addresses, so a seek past the last node of the first one would hit the second.
        let first = B256::with_last_byte(1);
        let second = B256::with_last_byte(2);
        // Zero runs that the memcmp encoding pads with, across its 8 nibble group boundary.
        let initial = [zeros(1), zeros(2), zeros(8), zeros(9), path(&[0, 1]), path(&[1])];
        let mut expected = BTreeMap::new();
        for address in [first, second] {
            for path in &initial {
                expected.insert((address, *path), node(1));
            }
        }

        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let cursor = provider.tx_ref().cursor_dup_write::<A::StorageTrieTable>().unwrap();
        let mut cursor = DatabaseStorageTrieCursor::<_, A>::new(cursor, first);
        for address in [first, second] {
            cursor.set_hashed_address(address);
            let nodes = initial.iter().map(|path| (*path, Some(node(1)))).collect();
            cursor.write_storage_trie_updates_sorted(&updates(nodes)).unwrap();
        }

        // Overwrite with a larger node, delete present and absent nodes, and insert after a
        // group boundary, reusing the cursor across addresses like the provider does.
        let mut extended = zeros(9);
        extended.push(1);
        cursor.set_hashed_address(first);
        cursor
            .write_storage_trie_updates_sorted(&updates(vec![
                (zeros(1), None),
                (zeros(2), Some(node(3))),
                (zeros(3), None),
                (extended, Some(node(2))),
            ]))
            .unwrap();
        cursor.set_hashed_address(second);
        cursor
            .write_storage_trie_updates_sorted(&updates(vec![
                (zeros(8), Some(node(16))),
                (path(&[1]), None),
            ]))
            .unwrap();
        expected.remove(&(first, zeros(1)));
        expected.insert((first, zeros(2)), node(3));
        expected.insert((first, extended), node(2));
        expected.insert((second, zeros(8)), node(16));
        expected.remove(&(second, path(&[1])));
        provider.commit().unwrap();

        let provider = factory.provider().unwrap();
        let trie_factory = DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref());
        let mut cursor = trie_factory.storage_trie_cursor(first).unwrap();
        let mut stored = BTreeMap::new();
        for address in [first, second] {
            cursor.set_hashed_address(address);
            let mut entry = cursor.seek(Nibbles::default()).unwrap();
            while let Some((path, node)) = entry {
                stored.insert((address, path), node);
                entry = cursor.next().unwrap();
            }
        }
        assert_eq!(stored, expected);

        cursor.set_hashed_address(first);
        assert_eq!(cursor.seek(path(&[2])).unwrap(), None);
        assert_eq!(cursor.seek_exact(zeros(1)).unwrap(), None);
        assert_eq!(cursor.seek(zeros(1)).unwrap().map(|(path, _)| path), Some(zeros(2)));
    }

    #[test]
    fn test_storage_trie_write_path_legacy() {
        storage_trie_write_path::<LegacyKeyAdapter>();
    }

    #[test]
    fn test_storage_trie_write_path_packed() {
        storage_trie_write_path::<PackedKeyAdapter>();
    }

    #[test]
    fn test_storage_trie_write_path_memcmp() {
        storage_trie_write_path::<MemcmpKeyAdapter>();
    }
}
