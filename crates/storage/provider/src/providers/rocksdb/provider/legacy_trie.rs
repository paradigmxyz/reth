//! Legacy hashed leaves and compact branch nodes over a committed `RocksDB` snapshot.
use super::{RocksDBBatch, RocksDBRawIterEnum, RocksReadSnapshot, RocksReadSnapshotInner};
use alloy_primitives::{B256, B512, U256};
use reth_db_api::{
    models::{state_trie::StateTrieStorageKey, CompactU256},
    table::{Decode, Decompress, Encode, Table},
    tables, DatabaseError,
};
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor},
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::TrieUpdatesSorted,
    BranchNodeCompact, HashedPostStateSorted, Nibbles, PackedStoredNibbles,
};
use std::{fmt, marker::PhantomData};

/// Snapshot-pinned cursor, optionally confined to one account's storage.
pub struct RocksLegacyCursor<'a, 'db, V> {
    snapshot: &'a RocksReadSnapshot<'db>,
    cf: &'db rocksdb::ColumnFamily,
    iter: Option<RocksDBRawIterEnum<'db>>,
    address: Option<B256>,
    marker: PhantomData<V>,
}

impl<V> fmt::Debug for RocksLegacyCursor<'_, '_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksLegacyCursor").field("address", &self.address).finish_non_exhaustive()
    }
}

impl<'a, 'db, V: Decompress> RocksLegacyCursor<'a, 'db, V> {
    fn new<T: Table>(
        snapshot: &'a RocksReadSnapshot<'db>,
        address: Option<B256>,
    ) -> Result<Self, DatabaseError> {
        Ok(Self {
            snapshot,
            cf: snapshot.cf_handle::<T>()?,
            iter: None,
            address,
            marker: PhantomData,
        })
    }

    fn iter(&mut self) -> &mut RocksDBRawIterEnum<'db> {
        self.iter.get_or_insert_with(|| self.snapshot.new_raw_iterator_cf(self.cf))
    }

    fn seek_key(&mut self, suffix: &[u8]) {
        let mut key = [0; 65];
        let offset = if let Some(address) = self.address {
            key[..32].copy_from_slice(address.as_slice());
            32
        } else {
            0
        };
        key[offset..offset + suffix.len()].copy_from_slice(suffix);
        self.iter().seek(&key[..offset + suffix.len()]);
    }

    fn current_key(&self) -> Result<Option<&[u8]>, DatabaseError> {
        let Some(iter) = &self.iter else { return Ok(None) };
        iter.status().map_err(|e| DatabaseError::Other(e.to_string()))?;
        let Some(key) = iter.key() else { return Ok(None) };
        if let Some(address) = self.address {
            Ok(key.strip_prefix(address.as_slice()))
        } else {
            Ok(Some(key))
        }
    }

    fn value(&self) -> Result<V, DatabaseError> {
        V::decompress(self.iter.as_ref().and_then(|i| i.value()).ok_or(DatabaseError::Decode)?)
            .map_err(|_| DatabaseError::Decode)
    }
}

macro_rules! impl_hashed_cursor {
    ($stored:ty, $value:ty, $decode:expr) => {
        impl HashedCursor for RocksLegacyCursor<'_, '_, $stored> {
            type Value = $value;

            fn seek(&mut self, key: B256) -> Result<Option<(B256, $value)>, DatabaseError> {
                self.seek_key(key.as_slice());
                self.current_key()?
                    .map(|key| Ok((B256::decode(key)?, ($decode)(self.value()?))))
                    .transpose()
            }

            fn next(&mut self) -> Result<Option<(B256, $value)>, DatabaseError> {
                // An exhausted storage cursor must never enter the next account.
                if self.current_key()?.is_none() {
                    return Ok(None)
                }
                self.iter().next();
                self.current_key()?
                    .map(|key| Ok((B256::decode(key)?, ($decode)(self.value()?))))
                    .transpose()
            }

            fn reset(&mut self) {
                self.iter = None;
            }
        }
    };
}
impl_hashed_cursor!(Account, Account, |value| value);
impl_hashed_cursor!(CompactU256, U256, |value: CompactU256| value.0);

impl HashedStorageCursor for RocksLegacyCursor<'_, '_, CompactU256> {
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        Ok(HashedCursor::seek(self, B256::ZERO)?.is_none())
    }
    fn set_hashed_address(&mut self, address: B256) {
        self.address = Some(address);
        self.iter = None;
    }
}

impl TrieCursor for RocksLegacyCursor<'_, '_, BranchNodeCompact> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(TrieCursor::seek(self, key)?.filter(|(path, _)| *path == key))
    }
    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.seek_key(PackedStoredNibbles(key).encode().as_ref());
        self.current_key()?
            .map(|key| Ok((PackedStoredNibbles::decode(key)?.0, self.value()?)))
            .transpose()
    }
    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if self.current_key()?.is_none() {
            return Ok(None)
        }
        self.iter().next();
        self.current_key()?
            .map(|key| Ok((PackedStoredNibbles::decode(key)?.0, self.value()?)))
            .transpose()
    }
    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        self.current_key()?.map(|key| PackedStoredNibbles::decode(key).map(|p| p.0)).transpose()
    }
    fn reset(&mut self) {
        self.iter = None;
    }
}

impl TrieStorageCursor for RocksLegacyCursor<'_, '_, BranchNodeCompact> {
    fn set_hashed_address(&mut self, address: B256) {
        self.address = Some(address);
        self.iter = None;
    }
}

/// Combine an account hash and a slot hash into the flattened storage key.
pub fn legacy_storage_key(address: B256, slot: B256) -> B512 {
    let mut bytes = [0; 64];
    bytes[..32].copy_from_slice(address.as_slice());
    bytes[32..].copy_from_slice(slot.as_slice());
    B512::from(bytes)
}

impl<'db> HashedCursorFactory for RocksReadSnapshot<'db> {
    type AccountCursor<'a>
        = RocksLegacyCursor<'a, 'db, Account>
    where
        Self: 'a;
    type StorageCursor<'a>
        = RocksLegacyCursor<'a, 'db, CompactU256>
    where
        Self: 'a;
    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        RocksLegacyCursor::new::<tables::HashedAccounts>(self, None)
    }
    fn hashed_storage_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        RocksLegacyCursor::new::<tables::RocksHashedStorages>(self, Some(address))
    }
    fn hashed_account(&self, address: B256) -> Result<Option<Account>, DatabaseError> {
        self.legacy_get::<tables::HashedAccounts>(address)
    }
    fn hashed_storage(&self, address: B256, slot: B256) -> Result<Option<U256>, DatabaseError> {
        self.legacy_get::<tables::RocksHashedStorages>(legacy_storage_key(address, slot))
            .map(|v| v.map(|v| v.0))
    }
    fn hashed_storage_batch(
        &self,
        address: B256,
        slots: &[B256],
    ) -> Result<Vec<Option<U256>>, DatabaseError> {
        let cf = self.cf_handle::<tables::RocksHashedStorages>()?;
        let keys: Vec<_> = slots.iter().map(|slot| legacy_storage_key(address, *slot)).collect();
        let values = match &self.inner {
            RocksReadSnapshotInner::ReadWrite(_) => {
                self.provider.db_rw().batched_multi_get_cf_opt(cf, &keys, false, &self.read_options)
            }
            RocksReadSnapshotInner::Secondary(db) => {
                db.batched_multi_get_cf_opt(cf, &keys, false, &self.read_options)
            }
        };
        values
            .into_iter()
            .map(|v| {
                v.map_err(|e| DatabaseError::Other(e.to_string()))?
                    .map(|v| {
                        CompactU256::decompress(&v).map(|v| v.0).map_err(|_| DatabaseError::Decode)
                    })
                    .transpose()
            })
            .collect()
    }
}

impl RocksReadSnapshot<'_> {
    fn legacy_get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        let cf = self.cf_handle::<T>()?;
        let key = key.encode();
        let value = match &self.inner {
            RocksReadSnapshotInner::ReadWrite(_) => {
                self.provider.db_rw().get_pinned_cf_opt(cf, key.as_ref(), &self.read_options)
            }
            RocksReadSnapshotInner::Secondary(db) => {
                db.get_pinned_cf_opt(cf, key.as_ref(), &self.read_options)
            }
        }
        .map_err(|e| DatabaseError::Other(e.to_string()))?;
        value.map(|v| T::Value::decompress(&v).map_err(|_| DatabaseError::Decode)).transpose()
    }
}

impl<'db> TrieCursorFactory for RocksReadSnapshot<'db> {
    type AccountTrieCursor<'a>
        = RocksLegacyCursor<'a, 'db, BranchNodeCompact>
    where
        Self: 'a;
    type StorageTrieCursor<'a>
        = RocksLegacyCursor<'a, 'db, BranchNodeCompact>
    where
        Self: 'a;
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        RocksLegacyCursor::new::<tables::RocksAccountsTrie>(self, None)
    }
    fn storage_trie_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        RocksLegacyCursor::new::<tables::RocksStoragesTrie>(self, Some(address))
    }
}

impl RocksDBBatch<'_> {
    /// Stage legacy hashed leaves; zero storage values remove existing keys.
    pub fn write_legacy_hashed_state(
        &mut self,
        state: &HashedPostStateSorted,
    ) -> ProviderResult<()> {
        for (address, account) in state.accounts() {
            match account {
                Some(account) => self.put::<tables::HashedAccounts>(*address, account)?,
                None => self.delete::<tables::HashedAccounts>(*address)?,
            }
        }
        for (address, storage) in state.account_storages() {
            for (slot, value) in storage.storage_slots_ref() {
                let key = legacy_storage_key(*address, *slot);
                if value.is_zero() {
                    self.delete::<tables::RocksHashedStorages>(key)?;
                } else {
                    self.put::<tables::RocksHashedStorages>(key, &CompactU256(*value))?;
                }
            }
        }
        Ok(())
    }

    /// Stage compact legacy branch updates after state masking.
    pub fn write_legacy_trie_updates(
        &mut self,
        updates: &TrieUpdatesSorted,
    ) -> ProviderResult<usize> {
        let mut count = 0;
        for (path, node) in updates.account_nodes_ref() {
            if path.is_empty() && node.is_some() {
                continue
            }
            match node {
                Some(node) => self.put::<tables::RocksAccountsTrie>((*path).into(), node)?,
                None => self.delete::<tables::RocksAccountsTrie>((*path).into())?,
            }
            count += 1;
        }
        for (address, storage) in updates.storage_tries_ref() {
            for (path, node) in storage.storage_nodes_ref() {
                if path.is_empty() {
                    continue
                }
                let key = StateTrieStorageKey { address: *address, path: (*path).into() };
                match node {
                    Some(node) => self.put::<tables::RocksStoragesTrie>(key, node)?,
                    None => self.delete::<tables::RocksStoragesTrie>(key)?,
                }
                count += 1;
            }
        }
        Ok(count)
    }
}

impl super::RocksDBProvider {
    /// Import a sorted legacy table through bounded SST files, then verify every encoded record.
    /// The destination must be empty. Callers must validate the persisted frontier first.
    pub fn import_legacy_table<T: Table>(
        &self,
        entries: impl Iterator<Item = Result<(T::Key, T::Value), DatabaseError>>,
        directory: &std::path::Path,
    ) -> ProviderResult<(u64, B256)> {
        use alloy_primitives::Keccak256;
        use reth_db_api::table::Compress;
        use reth_storage_errors::provider::ProviderError;
        use rocksdb::{Cache, IngestExternalFileOptions, SstFileWriter};
        if self.first::<T>()?.is_some() {
            return Err(ProviderError::other(std::io::Error::other(format!(
                "{} must be empty before import",
                T::NAME
            ))))
        }
        reth_fs_util::create_dir_all(directory).map_err(ProviderError::other)?;
        let options = super::RocksDBBuilder::state_trie_column_family_options(
            &Cache::new_lru_cache(64 << 20),
            true,
        );
        let cf = self.get_cf_handle::<T>()?;
        let mut ingestion = IngestExternalFileOptions::default();
        ingestion.set_move_files(true);
        let mut entries = entries.peekable();
        let mut digest = Keccak256::new();
        let mut count = 0u64;
        let mut buf = Vec::new();
        while entries.peek().is_some() {
            let path = directory.join(format!("{}-{count}.sst", T::NAME));
            let mut writer = SstFileWriter::create(&options);
            writer.open(&path).map_err(ProviderError::other)?;
            let mut bytes = 0;
            for entry in entries.by_ref() {
                let (key, value) = entry?;
                let key = key.encode();
                buf.clear();
                value.compress_to_buf(&mut buf);
                hash_legacy_record(&mut digest, key.as_ref(), &buf);
                writer.put(key.as_ref(), &buf).map_err(ProviderError::other)?;
                count += 1;
                bytes += key.as_ref().len() + buf.len();
                if bytes >= 256 << 20 {
                    break
                }
            }
            writer.finish().map_err(ProviderError::other)?;
            drop(writer);
            self.0
                .db_rw()
                .ingest_external_file_cf_opts(cf, &ingestion, vec![path])
                .map_err(ProviderError::other)?;
            eprintln!("legacy_table={} imported_records={count}", T::NAME);
        }
        let expected = digest.finalize();
        let actual = self.legacy_table_digest::<T>()?;
        if actual != (count, expected) {
            return Err(ProviderError::other(std::io::Error::other(format!(
                "{} record verification failed: {actual:?} != {:?}",
                T::NAME,
                (count, expected)
            ))))
        }
        Ok(actual)
    }

    /// Count and hash all raw records in a legacy table in key order.
    pub fn legacy_table_digest<T: Table>(&self) -> ProviderResult<(u64, B256)> {
        let mut digest = alloy_primitives::Keccak256::new();
        let mut count = 0;
        for entry in self.raw_iter::<T>()? {
            let (key, value) = entry?;
            hash_legacy_record(&mut digest, &key, &value);
            count += 1;
        }
        Ok((count, digest.finalize()))
    }
}

fn hash_legacy_record(hash: &mut alloy_primitives::Keccak256, key: &[u8], value: &[u8]) {
    hash.update((key.len() as u64).to_be_bytes());
    hash.update(key);
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use reth_db_api::{
        database::Database,
        transaction::{DbTx, DbTxMut},
    };
    use reth_trie::{
        proof_v2::{ProofCalculator, SyncAccountValueEncoder},
        ProofV2Target, StateRoot, StorageTrieEntry,
    };
    use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter};

    #[test]
    fn legacy_rocksdb_matches_mdbx_cursors_proofs_and_bulk_import() {
        let dir = tempfile::tempdir().unwrap();
        let rocks = super::super::RocksDBProvider::builder(dir.path().join("rocks"))
            .with_default_tables()
            .with_table::<tables::HashedAccounts>()
            .with_table::<tables::RocksHashedStorages>()
            .with_table::<tables::RocksAccountsTrie>()
            .with_table::<tables::RocksStoragesTrie>()
            .build()
            .unwrap();
        let db = reth_db::test_utils::create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        let addresses = [
            B256::ZERO,
            B256::with_last_byte(1),
            B256::with_last_byte(2),
            B256::repeat_byte(127),
            B256::repeat_byte(255),
        ];
        for address in addresses {
            tx.put::<tables::HashedAccounts>(address, Account { nonce: 3, ..Default::default() })
                .unwrap();
            for i in 1u64..100 {
                let mut slot = keccak256(i.to_be_bytes());
                slot.0[..2].fill(0);
                tx.put::<tables::HashedStorages>(
                    address,
                    reth_primitives_traits::StorageEntry { key: slot, value: U256::from(i) },
                )
                .unwrap();
            }
        }
        let hashed = DatabaseHashedCursorFactory::new(&tx);
        let trie = DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx);
        let (expected, updates) = StateRoot::new(trie, hashed).root_with_updates().unwrap();
        let updates = updates.into_sorted();
        for (path, node) in updates.account_nodes_ref() {
            if let Some(node) = node &&
                !path.is_empty()
            {
                tx.put::<tables::AccountsTrie>((*path).into(), node.clone()).unwrap();
            }
        }
        for (address, updates) in updates.storage_tries_ref() {
            for (path, node) in updates.storage_nodes_ref() {
                if let Some(node) = node &&
                    !path.is_empty()
                {
                    tx.put::<tables::StoragesTrie>(
                        *address,
                        StorageTrieEntry { nibbles: (*path).into(), node: node.clone() },
                    )
                    .unwrap();
                }
            }
        }
        tx.commit().unwrap();
        let tx = db.tx().unwrap();
        let work = dir.path().join("import");
        use reth_db_api::cursor::DbCursorRO;
        let mut accounts = tx.cursor_read::<tables::HashedAccounts>().unwrap();
        let result = rocks
            .import_legacy_table::<tables::HashedAccounts>(accounts.walk(None).unwrap(), &work)
            .unwrap();
        assert_eq!(result.0, addresses.len() as u64);
        assert!(rocks
            .import_legacy_table::<tables::HashedAccounts>(std::iter::empty(), &work)
            .is_err());
        let mut storage = tx.cursor_read::<tables::HashedStorages>().unwrap();
        rocks
            .import_legacy_table::<tables::RocksHashedStorages>(
                storage.walk(None).unwrap().map(|row| {
                    row.map(|(a, s)| (legacy_storage_key(a, s.key), CompactU256(s.value)))
                }),
                &work,
            )
            .unwrap();
        let mut batch = rocks.batch();
        batch.write_legacy_trie_updates(&updates).unwrap();
        batch.commit().unwrap();
        let snapshot = rocks.snapshot();
        let hashed = DatabaseHashedCursorFactory::new(&tx);
        let trie = DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx);
        let mut rocks_calc = ProofCalculator::new(
            snapshot.account_trie_cursor().unwrap(),
            snapshot.hashed_account_cursor().unwrap(),
        );
        let mut encoder = SyncAccountValueEncoder::new(&snapshot, &snapshot);
        let node = rocks_calc.root_node(&mut encoder).unwrap();
        assert_eq!(rocks_calc.compute_root_hash(&[node]).unwrap(), Some(expected));
        let mut mdbx_calc = ProofCalculator::new(
            trie.account_trie_cursor().unwrap(),
            hashed.hashed_account_cursor().unwrap(),
        );
        let mut mdbx_encoder = SyncAccountValueEncoder::new(&trie, &hashed);
        for address in addresses.into_iter().chain([B256::with_last_byte(1)]) {
            let target = ProofV2Target::new(address);
            assert_eq!(
                rocks_calc.proof(&mut encoder, &mut [target]).unwrap(),
                mdbx_calc.proof(&mut mdbx_encoder, &mut [target]).unwrap()
            );
            let mut a = snapshot.hashed_storage_cursor(address).unwrap();
            let mut b = hashed.hashed_storage_cursor(address).unwrap();
            assert_eq!(a.is_storage_empty().unwrap(), b.is_storage_empty().unwrap());
            assert_eq!(a.seek(B256::ZERO).unwrap(), b.seek(B256::ZERO).unwrap());
            for _ in 0..101 {
                assert_eq!(a.next().unwrap(), b.next().unwrap());
            }
            a.set_hashed_address(B256::ZERO);
            b.set_hashed_address(B256::ZERO);
            assert_eq!(a.seek(B256::ZERO).unwrap(), b.seek(B256::ZERO).unwrap());
            let mut a = ProofCalculator::new_storage(
                snapshot.storage_trie_cursor(address).unwrap(),
                snapshot.hashed_storage_cursor(address).unwrap(),
            );
            let mut b = ProofCalculator::new_storage(
                trie.storage_trie_cursor(address).unwrap(),
                hashed.hashed_storage_cursor(address).unwrap(),
            );
            let mut present = keccak256(1u64.to_be_bytes());
            present.0[..2].fill(0);
            for key in [B256::ZERO, B256::repeat_byte(255), present] {
                let target = ProofV2Target::new(key);
                assert_eq!(
                    a.storage_proof(address, &mut [target]).unwrap(),
                    b.storage_proof(address, &mut [target]).unwrap()
                );
            }
        }
        // Legacy MDBX ignores root inserts but still applies explicit root deletions.
        let root_path = Nibbles::new();
        rocks
            .put::<tables::RocksAccountsTrie>(
                root_path.into(),
                &BranchNodeCompact::new(3, 0, 0, vec![], None),
            )
            .unwrap();
        let mut batch = rocks.batch();
        batch
            .write_legacy_trie_updates(&TrieUpdatesSorted::new(
                vec![(root_path, None)],
                Default::default(),
            ))
            .unwrap();
        batch.commit().unwrap();
        assert_eq!(
            rocks.snapshot().account_trie_cursor().unwrap().seek_exact(root_path).unwrap(),
            None
        );
    }
}
