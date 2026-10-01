//! Complete state trie cursors and writes over `RocksDB` snapshots.
use super::{RocksDBBatch, RocksDBRawIterEnum, RocksReadSnapshot, RocksReadSnapshotInner};
use alloy_primitives::{B256, U256};
use reth_db_api::{
    models::state_trie::UnifiedStateTrieStorageKey,
    table::{Decode, Decompress, Encode, Table},
    tables, DatabaseError,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    state_trie_cursor::{
        StateTrieCursor, StateTrieCursorFactory, StateTrieCursorResult, StateTrieStorageCursor,
    },
    Nibbles, PackedStoredNibbles, StateTrieNode, StateTrieUpdatesSorted, TrieAccount,
};
use std::{fmt, marker::PhantomData};

#[cfg(feature = "state-trie-rocksdb")]
use reth_db_api::table::Compress;
#[cfg(feature = "state-trie-rocksdb")]
use rocksdb::SstFileWriter;

/// A cursor pinned to its provider's state trie snapshot.
/// Parent lookups use `RocksDB` Get; only neighbor lookups reposition the iterator.
pub struct RocksStateTrieCursor<'a, 'db, V> {
    snapshot: &'a RocksReadSnapshot<'db>,
    cf: &'a rocksdb::ColumnFamily,
    iter: Option<RocksDBRawIterEnum<'db>>,
    address: Option<B256>,
    marker: PhantomData<V>,
}

impl<V> fmt::Debug for RocksStateTrieCursor<'_, '_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksStateTrieCursor")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl<'a, 'db, V> RocksStateTrieCursor<'a, 'db, V> {
    fn new<T: Table>(
        snapshot: &'a RocksReadSnapshot<'db>,
        address: Option<B256>,
    ) -> Result<Self, DatabaseError> {
        let cf = snapshot.cf_handle::<T>()?;
        Ok(Self { snapshot, cf, iter: None, address, marker: PhantomData })
    }

    fn iter(&mut self) -> &mut RocksDBRawIterEnum<'db> {
        self.iter.get_or_insert_with(|| self.snapshot.new_raw_iterator_cf(self.cf))
    }

    fn key(&self, path: Nibbles) -> ([u8; 66], usize) {
        let mut key = [0; 66];
        let offset = if let Some(address) = self.address {
            key[..32].copy_from_slice(address.as_slice());
            key[32] = 64;
            33
        } else {
            0
        };
        key[offset..offset + 33].copy_from_slice(PackedStoredNibbles(path).encode().as_ref());
        (key, offset + 33)
    }
}

impl<V: Clone + fmt::Debug> RocksStateTrieCursor<'_, '_, V>
where
    StateTrieNode<V>: Decompress,
{
    fn account_neighbor(&mut self, backwards: bool) -> Result<(), DatabaseError> {
        if self.address.is_some() {
            return Ok(())
        }
        while let Some(key) = self.iter().key() {
            if key.len() == 33 {
                break
            }
            if key.len() != 66 || key[32] != 64 {
                return Err(DatabaseError::Decode)
            }
            let mut account = [0; 33];
            account.copy_from_slice(&key[..33]);
            if backwards {
                self.iter().seek_for_prev(account);
            } else {
                // All storage keys extend this account key; the next length byte skips them.
                account[32] = 65;
                self.iter().seek(account);
            }
        }
        Ok(())
    }

    fn current(&self) -> StateTrieCursorResult<V> {
        let Some(iter) = &self.iter else { return Ok(None) };
        iter.status().map_err(|e| DatabaseError::Other(e.to_string()))?;
        let Some(key) = iter.key() else { return Ok(None) };
        let key = if let Some(address) = self.address {
            if key.len() != 66 || key[32] != 64 || !key.starts_with(address.as_slice()) {
                return Ok(None)
            }
            &key[33..]
        } else {
            key
        };
        let path = PackedStoredNibbles::decode(key)?.0;
        let value = iter.value().ok_or(DatabaseError::Decode)?;
        Ok(Some((path, StateTrieNode::decompress(value).map_err(|_| DatabaseError::Decode)?)))
    }
}

impl<V: Clone + fmt::Debug> StateTrieCursor for RocksStateTrieCursor<'_, '_, V>
where
    StateTrieNode<V>: Decompress,
{
    type Value = V;

    fn get(&mut self, path: Nibbles) -> Result<Option<StateTrieNode<V>>, DatabaseError> {
        let (key, len) = self.key(path);
        let value = match &self.snapshot.inner {
            RocksReadSnapshotInner::ReadWrite(_) => self
                .snapshot
                .provider
                .db_rw()
                .get_pinned_cf_opt(self.cf, &key[..len], &self.snapshot.read_options),
            RocksReadSnapshotInner::Secondary(db) => {
                db.get_pinned_cf_opt(self.cf, &key[..len], &self.snapshot.read_options)
            }
        }
        .map_err(|e| DatabaseError::Other(e.to_string()))?;
        value
            .map(|bytes| StateTrieNode::decompress(&bytes).map_err(|_| DatabaseError::Decode))
            .transpose()
    }

    fn get_batch(
        &mut self,
        paths: &[Nibbles],
    ) -> Result<Vec<Option<StateTrieNode<V>>>, DatabaseError> {
        if paths.is_empty() {
            return Ok(Vec::new())
        }
        if let [path] = paths {
            return Ok(vec![self.get(*path)?])
        }
        let encoded: Vec<_> = paths.iter().map(|path| self.key(*path)).collect();
        let keys = encoded.iter().map(|(key, len)| &key[..*len]);
        let values = match &self.snapshot.inner {
            RocksReadSnapshotInner::ReadWrite(_) => self
                .snapshot
                .provider
                .db_rw()
                .batched_multi_get_cf_opt(self.cf, keys, false, &self.snapshot.read_options),
            RocksReadSnapshotInner::Secondary(db) => {
                db.batched_multi_get_cf_opt(self.cf, keys, false, &self.snapshot.read_options)
            }
        };
        values
            .into_iter()
            .map(|value| {
                value
                    .map_err(|e| DatabaseError::Other(e.to_string()))?
                    .map(|bytes| {
                        StateTrieNode::decompress(&bytes).map_err(|_| DatabaseError::Decode)
                    })
                    .transpose()
            })
            .collect()
    }

    fn seek(&mut self, path: Nibbles) -> StateTrieCursorResult<V> {
        // Existing proof targets can use Bloom filters and avoid merging SST iterators.
        if let Some(node) = self.get(path)? {
            return Ok(Some((path, node)))
        }
        let (key, len) = self.key(path);
        self.iter().seek(&key[..len]);
        self.account_neighbor(false)?;
        self.current()
    }

    fn before(&mut self, path: Option<Nibbles>) -> StateTrieCursorResult<V> {
        if let Some(path) = path {
            let (key, len) = self.key(path);
            self.iter().seek_for_prev(&key[..len]);
            if self.iter().key() == Some(&key[..len]) {
                self.iter().prev();
            }
        } else if let Some(address) = self.address {
            let mut end = [0xff; 66];
            end[..32].copy_from_slice(address.as_slice());
            end[32] = 64;
            self.iter().seek_for_prev(end);
        } else {
            self.iter().seek_to_last();
        }
        self.account_neighbor(true)?;
        self.current()
    }
}

impl StateTrieStorageCursor for RocksStateTrieCursor<'_, '_, U256> {
    fn set_hashed_address(&mut self, address: B256) {
        self.address = Some(address);
    }
}

impl<'db> StateTrieCursorFactory for RocksReadSnapshot<'db> {
    type AccountCursor<'a>
        = RocksStateTrieCursor<'a, 'db, TrieAccount>
    where
        Self: 'a;
    type StorageCursor<'a>
        = RocksStateTrieCursor<'a, 'db, U256>
    where
        Self: 'a;

    fn state_trie_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        RocksStateTrieCursor::new::<tables::UnifiedStateTrieAccounts>(self, None)
    }

    fn state_trie_storage_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        RocksStateTrieCursor::new::<tables::UnifiedStateTrieStorages>(self, Some(address))
    }
}

impl RocksDBBatch<'_> {
    /// Add complete trie updates to the batch without committing it.
    pub fn write_state_trie_updates(
        &mut self,
        updates: &StateTrieUpdatesSorted,
    ) -> ProviderResult<()> {
        for (path, node) in &updates.account_nodes {
            match node {
                Some(node) => self.put::<tables::UnifiedStateTrieAccounts>((*path).into(), node)?,
                None => self.delete::<tables::UnifiedStateTrieAccounts>((*path).into())?,
            }
        }
        for (address, nodes) in &updates.storage_tries {
            for (path, node) in nodes {
                let key = UnifiedStateTrieStorageKey { address: *address, path: (*path).into() };
                match node {
                    Some(node) => self.put::<tables::UnifiedStateTrieStorages>(key, node)?,
                    None => self.delete::<tables::UnifiedStateTrieStorages>(key)?,
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "state-trie-rocksdb")]
impl super::RocksDBProvider {
    /// Builds immutable update files without making them visible to readers.
    pub(crate) fn stage_state_trie_ssts(
        &self,
        updates: &StateTrieUpdatesSorted,
    ) -> ProviderResult<tempfile::TempDir> {
        let dir = tempfile::Builder::new()
            .prefix("state-trie-")
            .tempdir_in(self.0.path())
            .map_err(reth_storage_errors::provider::ProviderError::other)?;
        let options = super::RocksDBBuilder::state_trie_column_family_options(
            &rocksdb::Cache::new_lru_cache(0),
            false,
        );
        let mut storages: Vec<_> =
            updates.storage_tries.iter().filter(|(_, nodes)| !nodes.is_empty()).collect();
        if updates.account_nodes.is_empty() && storages.is_empty() {
            return Ok(dir)
        }
        storages.sort_unstable_by_key(|(address, _)| **address);
        let write = || -> Result<(), rocksdb::Error> {
            let mut writer = SstFileWriter::create(&options);
            writer.open(dir.path().join("state.sst"))?;
            let mut buf = Vec::new();
            let mut accounts = updates.account_nodes.iter().peekable();
            for (address, nodes) in storages {
                let prefix = PackedStoredNibbles(Nibbles::unpack(address)).encode();
                while accounts
                    .peek()
                    .is_some_and(|(path, _)| PackedStoredNibbles(*path).encode() <= prefix)
                {
                    let (path, node) = accounts.next().expect("peeked account");
                    write_node(
                        &mut writer,
                        &mut buf,
                        &PackedStoredNibbles(*path).encode(),
                        node.as_ref(),
                    )?;
                }
                for (path, node) in nodes {
                    let key =
                        UnifiedStateTrieStorageKey { address: *address, path: (*path).into() }
                            .encode();
                    write_node(&mut writer, &mut buf, &key, node.as_ref())?;
                }
            }
            for (path, node) in accounts {
                write_node(
                    &mut writer,
                    &mut buf,
                    &PackedStoredNibbles(*path).encode(),
                    node.as_ref(),
                )?;
            }
            writer.finish()
        };
        write().map_err(|e| DatabaseError::Other(e.to_string()))?;
        // SstFileWriter evicts its output from the OS page cache. These files become
        // active state immediately, so load them before publishing the new snapshot.
        let warm = |name| -> ProviderResult<u64> {
            let path = dir.path().join(name);
            if !path.try_exists().map_err(reth_storage_errors::provider::ProviderError::other)? {
                return Ok(0)
            }
            let file = reth_fs_util::open(&path)
                .map_err(reth_storage_errors::provider::ProviderError::other)?;
            let mut reader = std::io::BufReader::with_capacity(1 << 20, file);
            std::io::copy(&mut reader, &mut std::io::sink())
                .map_err(reth_storage_errors::provider::ProviderError::other)
        };
        let bytes = warm("state.sst")?;
        tracing::debug!(target: "engine::persistence", state_trie_sst_bytes = bytes, "Staged state trie SST files");
        Ok(dir)
    }

    /// Installs durable updates before the caller publishes its MDBX checkpoint and snapshot.
    pub(crate) fn ingest_state_trie_ssts(
        &self,
        files: Vec<tempfile::TempDir>,
    ) -> ProviderResult<()> {
        let mut options = rocksdb::IngestExternalFileOptions::default();
        options.set_move_files(true);
        options.set_snapshot_consistency(true);
        options.set_allow_global_seqno(true);
        for dir in files {
            for (table, name) in [(tables::UnifiedStateTrieAccounts::NAME, "state.sst")] {
                let path = dir.path().join(name);
                if !path
                    .try_exists()
                    .map_err(reth_storage_errors::provider::ProviderError::other)?
                {
                    continue
                }
                let db = self.0.db_rw();
                let cf = db
                    .cf_handle(table)
                    .ok_or_else(|| DatabaseError::Other(format!("missing {table}")))?;
                db.ingest_external_file_cf_opts(cf, &options, vec![path])
                    .map_err(|e| DatabaseError::Other(e.to_string()))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use reth_db_api::{database::Database, transaction::DbTx};
    use reth_trie::{
        proof_v3::ProofCalculator, ProofV2Target, ProofV2TargetParent, StateTrieBuilder,
    };
    use reth_trie_db::{write_state_trie_updates, DatabaseStateTrieCursorFactory};
    use std::{collections::BTreeMap, convert::Infallible};

    fn compare_cursors<V: Clone + fmt::Debug + PartialEq>(
        mut rocks: impl StateTrieCursor<Value = V>,
        mut mdbx: impl StateTrieCursor<Value = V>,
        paths: impl IntoIterator<Item = Nibbles>,
    ) {
        assert_eq!(rocks.before(None).unwrap(), mdbx.before(None).unwrap());
        let paths: Vec<_> = paths.into_iter().collect();
        let batch: Vec<_> = paths.iter().rev().chain(paths.iter().take(2)).copied().collect();
        assert_eq!(rocks.get_batch(&[]).unwrap(), Vec::new());
        assert_eq!(
            rocks.get_batch(&batch).unwrap(),
            batch.iter().map(|path| mdbx.get(*path).unwrap()).collect::<Vec<_>>()
        );
        for path in paths {
            assert_eq!(rocks.get(path).unwrap(), mdbx.get(path).unwrap());
            assert_eq!(rocks.seek(path).unwrap(), mdbx.seek(path).unwrap());
            assert_eq!(rocks.before(Some(path)).unwrap(), mdbx.before(Some(path)).unwrap());
        }
    }

    #[cfg(feature = "state-trie-rocksdb")]
    #[test]
    fn state_trie_sst_snapshots_abort_and_process_exit() {
        const CHILD_PATH: &str = "RETH_STATE_TRIE_SST_TEST_PATH";
        let path = Nibbles::unpack(B256::repeat_byte(1));
        let removed = Nibbles::unpack(B256::repeat_byte(2));
        let address = B256::repeat_byte(3);
        let storage_key = UnifiedStateTrieStorageKey { address, path: path.into() };
        let old_slot = StateTrieNode::Leaf { short_key_len: 64, value: U256::from(7) };
        let account = |nonce| StateTrieNode::Leaf {
            short_key_len: 64,
            value: TrieAccount { nonce, ..Default::default() },
        };
        if let Some(dir) = std::env::var_os(CHILD_PATH) {
            let rocks = super::super::RocksDBProvider::builder(std::path::PathBuf::from(dir))
                .with_default_tables()
                .build()
                .unwrap();
            let mut batch = rocks.batch();
            for key in [path, removed] {
                batch.put::<tables::UnifiedStateTrieAccounts>(key.into(), &account(1)).unwrap();
            }
            batch.put::<tables::UnifiedStateTrieStorages>(storage_key, &old_slot).unwrap();
            batch.commit().unwrap();
            let old = rocks.snapshot();
            let updates = |nonce| StateTrieUpdatesSorted {
                account_nodes: vec![(path, Some(account(nonce))), (removed, None)],
                storage_tries: std::iter::once((
                    address,
                    vec![
                        (path, None),
                        (
                            removed,
                            Some(StateTrieNode::Leaf {
                                short_key_len: 64,
                                value: U256::from(nonce),
                            }),
                        ),
                    ],
                ))
                .collect(),
            };
            let empty = rocks.stage_state_trie_ssts(&StateTrieUpdatesSorted::default()).unwrap();
            rocks.ingest_state_trie_ssts(vec![empty]).unwrap();
            let discarded = rocks.stage_state_trie_ssts(&updates(9)).unwrap();
            let discarded_path = discarded.path().to_path_buf();
            drop(discarded);
            assert!(!discarded_path.exists());
            let first = rocks.stage_state_trie_ssts(&updates(2)).unwrap();
            let second = rocks.stage_state_trie_ssts(&updates(3)).unwrap();
            assert_eq!(
                rocks.get::<tables::UnifiedStateTrieAccounts>(path.into()).unwrap(),
                Some(account(1))
            );
            rocks.ingest_state_trie_ssts(vec![first, second]).unwrap();
            assert_eq!(
                old.state_trie_account_cursor().unwrap().get(path).unwrap(),
                Some(account(1))
            );
            assert_eq!(
                old.state_trie_account_cursor().unwrap().get(removed).unwrap(),
                Some(account(1))
            );
            assert_eq!(
                old.state_trie_storage_cursor(address).unwrap().get(path).unwrap(),
                Some(old_slot)
            );
            assert_eq!(old.state_trie_storage_cursor(address).unwrap().get(removed).unwrap(), None);
            assert_eq!(
                rocks.get::<tables::UnifiedStateTrieAccounts>(path.into()).unwrap(),
                Some(account(3))
            );
            assert_eq!(
                rocks.get::<tables::UnifiedStateTrieAccounts>(removed.into()).unwrap(),
                None
            );
            // No provider/destructor flush: ingestion must already be durable.
            std::process::exit(0);
        }
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "providers::rocksdb::provider::state_trie::tests::state_trie_sst_snapshots_abort_and_process_exit"])
            .env(CHILD_PATH, dir.path()).status().unwrap();
        assert!(status.success());
        let rocks = super::super::RocksDBProvider::builder(dir.path())
            .with_default_tables()
            .build()
            .unwrap();
        assert_eq!(
            rocks.get::<tables::UnifiedStateTrieAccounts>(path.into()).unwrap(),
            Some(account(3))
        );
        assert_eq!(rocks.get::<tables::UnifiedStateTrieAccounts>(removed.into()).unwrap(), None);
        assert_eq!(rocks.get::<tables::UnifiedStateTrieStorages>(storage_key).unwrap(), None);
        assert_eq!(
            rocks
                .get::<tables::UnifiedStateTrieStorages>(UnifiedStateTrieStorageKey {
                    address,
                    path: removed.into(),
                })
                .unwrap(),
            Some(StateTrieNode::Leaf { short_key_len: 64, value: U256::from(3) })
        );
    }

    #[test]
    fn state_trie_rocksdb_matches_mdbx_cursors_and_proofs() {
        let dir = tempfile::tempdir().unwrap();
        let rocks = super::super::RocksDBProvider::builder(dir.path())
            .with_default_tables()
            .build()
            .unwrap();
        let db = reth_db::test_utils::create_test_rw_db();
        let mut updates = StateTrieUpdatesSorted::default();
        let mut accounts = StateTrieBuilder::default();
        for (address, prefix_len) in [(B256::ZERO, 2), (B256::repeat_byte(255), 31)] {
            let mut storage = StateTrieBuilder::default();
            let mut nodes = BTreeMap::new();
            let mut write = |path, node| {
                nodes.insert(path, Some(node));
                Ok::<_, Infallible>(())
            };
            let leaves: BTreeMap<_, _> = (1u64..100)
                .map(|i| {
                    let mut key = keccak256(i.to_be_bytes());
                    key.0[..prefix_len].fill(0);
                    (key, U256::from(i))
                })
                .collect();
            for (key, value) in leaves {
                storage.push(key, value, &mut write).unwrap();
            }
            let storage_root = storage.finish(&mut write).unwrap();
            updates.storage_tries.insert(address, nodes.into_iter().collect());
            accounts
                .push(address, TrieAccount { storage_root, ..Default::default() }, &mut |p, n| {
                    updates.account_nodes.push((p, Some(n)));
                    Ok::<_, Infallible>(())
                })
                .unwrap();
        }
        let root = accounts
            .finish(&mut |p, n| {
                updates.account_nodes.push((p, Some(n)));
                Ok::<_, Infallible>(())
            })
            .unwrap();
        updates.account_nodes.sort_unstable_by_key(|(p, _)| *p);
        let tx = db.tx_mut().unwrap();
        write_state_trie_updates(&tx, &updates).unwrap();
        tx.commit().unwrap();
        #[cfg(feature = "state-trie-rocksdb")]
        {
            let files = rocks.stage_state_trie_ssts(&updates).unwrap();
            rocks.ingest_state_trie_ssts(vec![files]).unwrap();
        }
        #[cfg(not(feature = "state-trie-rocksdb"))]
        {
            let mut batch = rocks.batch();
            batch.write_state_trie_updates(&updates).unwrap();
            batch.commit().unwrap();
        }
        rocks.flush(&[tables::UnifiedStateTrieAccounts::NAME]).unwrap();
        let tx = db.tx().unwrap();
        let mdbx = DatabaseStateTrieCursorFactory(&tx);
        let snapshot = rocks.snapshot();
        let probes =
            (0u64..100).map(|i| Nibbles::unpack(keccak256(i.to_be_bytes()))).collect::<Vec<_>>();
        compare_cursors(
            snapshot.state_trie_account_cursor().unwrap(),
            mdbx.state_trie_account_cursor().unwrap(),
            updates
                .account_nodes
                .iter()
                .map(|(p, _)| *p)
                .chain(probes.iter().copied())
                .chain([Nibbles::new()]),
        );
        let mut calc = ProofCalculator::new(snapshot.state_trie_account_cursor().unwrap());
        let node = calc.root_node().unwrap();
        assert_eq!(calc.compute_root_hash(&[node]).unwrap(), Some(root));
        let mut reusable = snapshot.state_trie_storage_cursor(B256::ZERO).unwrap();
        for address in [B256::ZERO, B256::with_last_byte(1), B256::repeat_byte(255)] {
            reusable.set_hashed_address(address);
            assert_eq!(
                reusable.before(None).unwrap(),
                mdbx.state_trie_storage_cursor(address).unwrap().before(None).unwrap()
            );
            let paths = updates
                .storage_tries
                .get(&address)
                .into_iter()
                .flatten()
                .map(|(p, _)| *p)
                .chain(probes.iter().copied())
                .chain([
                    Nibbles::new(),
                    Nibbles::unpack(B256::ZERO),
                    Nibbles::unpack(B256::repeat_byte(255)),
                ])
                .collect::<Vec<_>>();
            compare_cursors(
                snapshot.state_trie_storage_cursor(address).unwrap(),
                mdbx.state_trie_storage_cursor(address).unwrap(),
                paths.iter().copied(),
            );
            let mut rocks_proof =
                ProofCalculator::new(snapshot.state_trie_storage_cursor(address).unwrap());
            let mut mdbx_proof =
                ProofCalculator::new(mdbx.state_trie_storage_cursor(address).unwrap());
            for path in paths.into_iter().filter(|p| p.len() == 64) {
                let mut targets = vec![ProofV2Target::new(B256::from_slice(&path.pack()))];
                if let Some(nodes) = updates.storage_tries.get(&address) {
                    for (p, node) in nodes {
                        if matches!(node, Some(StateTrieNode::Branch { .. })) && path.starts_with(p)
                        {
                            targets.push(targets[0].with_parent(ProofV2TargetParent::new(p.len())));
                        }
                    }
                }
                assert_eq!(
                    rocks_proof.proof(&mut targets).unwrap(),
                    mdbx_proof.proof(&mut targets).unwrap()
                );
            }
        }
    }

    #[test]
    fn state_trie_secondary_reads_follow_catch_up() {
        let dir = tempfile::tempdir().unwrap();
        let primary = super::super::RocksDBProvider::builder(dir.path())
            .with_default_tables()
            .build()
            .unwrap();
        let path = Nibbles::unpack(B256::ZERO);
        let leaf = |nonce| StateTrieNode::Leaf {
            short_key_len: 64,
            value: TrieAccount { nonce, ..Default::default() },
        };
        primary.put::<tables::UnifiedStateTrieAccounts>(path.into(), &leaf(1)).unwrap();
        primary.flush(&[tables::UnifiedStateTrieAccounts::NAME]).unwrap();
        let secondary = super::super::RocksDBProvider::builder(dir.path())
            .with_default_tables()
            .with_read_only(true)
            .build()
            .unwrap();
        let snapshot = secondary.snapshot();
        let mut cursor = snapshot.state_trie_account_cursor().unwrap();
        assert_eq!(cursor.get(path).unwrap(), Some(leaf(1)));
        assert_eq!(cursor.get_batch(&[path, path]).unwrap(), vec![Some(leaf(1)), Some(leaf(1))]);
        primary.put::<tables::UnifiedStateTrieAccounts>(path.into(), &leaf(2)).unwrap();
        primary.flush(&[tables::UnifiedStateTrieAccounts::NAME]).unwrap();
        secondary.try_catch_up_with_primary().unwrap();
        assert_eq!(cursor.get(path).unwrap(), Some(leaf(2)));
        assert_eq!(cursor.get_batch(&[path, path]).unwrap(), vec![Some(leaf(2)), Some(leaf(2))]);
    }

    #[test]
    fn state_trie_rocksdb_updates_snapshots_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = Nibbles::unpack(B256::ZERO);
        let leaf = StateTrieNode::Leaf { short_key_len: 64, value: U256::from(1) };
        let account = StateTrieNode::Leaf { short_key_len: 64, value: TrieAccount::default() };
        let rocks = super::super::RocksDBProvider::builder(dir.path())
            .with_default_tables()
            .build()
            .unwrap();
        let mut updates = StateTrieUpdatesSorted {
            account_nodes: vec![(path, Some(account.clone()))],
            storage_tries: std::iter::once((B256::ZERO, vec![(path, Some(leaf.clone()))]))
                .collect(),
        };
        let mut batch = rocks.batch();
        batch.write_state_trie_updates(&updates).unwrap();
        batch.commit().unwrap();
        rocks.flush(&[tables::UnifiedStateTrieAccounts::NAME]).unwrap();
        let snapshot = rocks.snapshot();
        let mut cursor = snapshot.state_trie_storage_cursor(B256::ZERO).unwrap();
        let paths = [path, Nibbles::unpack(B256::with_last_byte(1)), path];
        let original = vec![Some(leaf.clone()), None, Some(leaf.clone())];
        assert_eq!(cursor.get_batch(&paths).unwrap(), original);
        assert_eq!(cursor.get(path).unwrap(), Some(leaf.clone()));
        assert!(cursor.iter.is_none(), "point reads must not allocate an iterator");
        assert_eq!(cursor.seek(path).unwrap(), Some((path, leaf.clone())));
        updates.account_nodes[0].1 = None;
        let replacement = StateTrieNode::Leaf { short_key_len: 64, value: U256::from(2) };
        updates.storage_tries.get_mut(&B256::ZERO).unwrap()[0].1 = Some(replacement.clone());
        let mut batch = rocks.batch();
        batch.write_state_trie_updates(&updates).unwrap();
        batch.commit().unwrap();
        rocks.flush(&[tables::UnifiedStateTrieAccounts::NAME]).unwrap();
        assert_eq!(rocks.snapshot().state_trie_account_cursor().unwrap().get(path).unwrap(), None);
        assert_eq!(
            rocks.snapshot().state_trie_storage_cursor(B256::ZERO).unwrap().get(path).unwrap(),
            Some(replacement.clone())
        );
        assert_eq!(cursor.get(path).unwrap(), Some(leaf.clone()));
        assert_eq!(cursor.get_batch(&paths).unwrap(), original);
        assert_eq!(
            rocks
                .snapshot()
                .state_trie_storage_cursor(B256::ZERO)
                .unwrap()
                .get_batch(&paths)
                .unwrap(),
            vec![Some(replacement.clone()), None, Some(replacement.clone())]
        );
        assert_eq!(cursor.seek(path).unwrap(), Some((path, leaf)));
        assert_eq!(snapshot.state_trie_account_cursor().unwrap().get(path).unwrap(), Some(account));
        drop(cursor);
        drop(snapshot);
        drop(rocks);
        let rocks = super::super::RocksDBProvider::builder(dir.path())
            .with_default_tables()
            .build()
            .unwrap();
        assert_eq!(rocks.snapshot().state_trie_account_cursor().unwrap().get(path).unwrap(), None);
        assert_eq!(
            rocks.snapshot().state_trie_storage_cursor(B256::ZERO).unwrap().get(path).unwrap(),
            Some(replacement)
        );
        updates.storage_tries.get_mut(&B256::ZERO).unwrap()[0].1 = None;
        let mut batch = rocks.batch();
        batch.write_state_trie_updates(&updates).unwrap();
        batch.commit().unwrap();
        assert_eq!(
            rocks.snapshot().state_trie_storage_cursor(B256::ZERO).unwrap().before(None).unwrap(),
            None
        );
    }
}

#[cfg(feature = "state-trie-rocksdb")]
fn write_node(
    writer: &mut SstFileWriter<'_>,
    buf: &mut Vec<u8>,
    key: &[u8],
    node: Option<&impl Compress>,
) -> Result<(), rocksdb::Error> {
    if let Some(node) = node {
        buf.clear();
        node.compress_to_buf(buf);
        writer.put(key, buf)
    } else {
        writer.delete(key)
    }
}
