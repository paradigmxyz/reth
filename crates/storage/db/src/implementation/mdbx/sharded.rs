//! Four ordered duplicate-key prefix shards under a single MDBX parent transaction.
//!
//! Logical cursors preserve `(key, encoded value)` order. Point duplicate seeks touch
//! only the selected shard unless its suffix is empty. Whole-table scans merge shards;
//! persistence workers instead receive one exclusive physical cursor each.

use reth_libmdbx::{Cursor, Error, Result, TransactionKind, WriteFlags, RW};
use std::borrow::Cow;

/// Converts an offline v2 MDBX database to the experimental four-shard layout.
///
/// Both tables and a completion marker commit atomically. A durable, deliberately
/// incompatible version file fences normal node opens during migration. Rerunning
/// after interruption either retries the rolled-back transaction or publishes the
/// completed version; no partially migrated database is exposed to either binary.
/// This is intended for disposable benchmark snapshots, not production migrations.
pub fn migrate_storage_shards(path: &std::path::Path) -> eyre::Result<()> {
    use super::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind};
    use crate::{
        version::{db_version_file_path, get_db_version, DB_VERSION},
        Database,
    };
    use reth_db_api::{models::StorageSettings, tables::Metadata, transaction::DbTx};
    use std::io::Write;

    const MIGRATING: u64 = 3000003;
    let env = DatabaseEnv::open(
        path,
        DatabaseEnvKind::RW,
        DatabaseArguments::default().with_exclusive(Some(true)),
    )?;
    let version = get_db_version(path)?;
    if version == DB_VERSION {
        return Ok(())
    }
    eyre::ensure!(version == 2 || version == MIGRATING, "unsupported source DB version {version}");
    let settings = env
        .tx()?
        .get::<Metadata>("storage_settings".to_owned())?
        .ok_or_else(|| eyre::eyre!("missing storage_settings; refusing to guess trie encoding"))?;
    let settings: StorageSettings = serde_json::from_slice(&settings)?;
    reth_fs_util::atomic_write_file(&db_version_file_path(path), |f| {
        f.write_all(MIGRATING.to_string().as_bytes())
    })?;
    let tx = env.inner.begin_rw_txn()?;
    let marker =
        tx.create_db(Some("StorageShardMigrationV1"), reth_libmdbx::DatabaseFlags::default())?;
    if tx.get::<Vec<u8>>(marker.dbi(), b"complete")?.is_none() {
        for (names, shift) in [(&HASHED, 6), (&TRIE, if settings.is_v2() { 6 } else { 2 })] {
            let source = tx.open_db(Some(names[0]))?;
            let original_count = tx.db_stat(source.dbi())?.entries();
            let mut targets = Vec::new();
            for name in &names[1..] {
                let db = tx.create_db(Some(name), reth_libmdbx::DatabaseFlags::DUP_SORT)?;
                eyre::ensure!(
                    tx.db_stat(db.dbi())?.entries() == 0,
                    "nonempty target {name} without completion marker"
                );
                targets.push(db.dbi());
            }
            let mut cursor = tx.cursor_with_dbi(source.dbi())?;
            let mut row = cursor.first::<Vec<u8>, Vec<u8>>()?;
            let mut moved = 0usize;
            while let Some((key, value)) = row {
                let shard = usize::from(value[0] >> shift);
                eyre::ensure!(shard < 4, "invalid storage prefix in {}", names[0]);
                if shard != 0 {
                    tx.put(targets[shard - 1], &key, &value, WriteFlags::UPSERT)?;
                    cursor.del(WriteFlags::CURRENT)?;
                    moved += 1;
                }
                row = cursor.next()?;
            }
            drop(cursor);
            let mut count = tx.db_stat(source.dbi())?.entries();
            for dbi in targets {
                count += tx.db_stat(dbi)?.entries();
            }
            eyre::ensure!(count == original_count, "migration row count mismatch in {}", names[0]);
            tracing::info!(table = names[0], original_count, moved, "Migrated prefix shards");
        }
        tx.put(marker.dbi(), b"complete", b"1", WriteFlags::UPSERT)?;
    }
    tx.commit()?;
    reth_fs_util::atomic_write_file(&db_version_file_path(path), |f| {
        f.write_all(DB_VERSION.to_string().as_bytes())
    })?;
    Ok(())
}

pub(super) const HASHED: [&str; 4] =
    ["HashedStorages", "HashedStoragesShard1", "HashedStoragesShard2", "HashedStoragesShard3"];
pub(super) const TRIE: [&str; 4] =
    ["StoragesTrie", "StoragesTrieShard1", "StoragesTrieShard2", "StoragesTrieShard3"];

pub(super) fn names(name: &str) -> Option<&'static [&'static str; 4]> {
    match name {
        "HashedStorages" => Some(&HASHED),
        "StoragesTrie" => Some(&TRIE),
        _ => None,
    }
}

type Row = (Vec<u8>, Vec<u8>);
type Pair<'a> = Result<Option<(Cow<'a, [u8]>, Cow<'a, [u8]>)>>;
type Value<'a> = Result<Option<Cow<'a, [u8]>>>;

fn pair<'a>(row: Option<Row>) -> Pair<'a> {
    Ok(row.map(|(k, v)| (Cow::Owned(k), Cow::Owned(v))))
}

/// Raw cursor facade shared by typed logical cursors and exclusive shard cursors.
#[derive(Debug)]
pub(super) struct ShardedCursor<K: TransactionKind> {
    cursors: Vec<Cursor<K>>,
    shift: u8,
    current: Option<(usize, Row)>,
    deleted: bool,
}

impl<K: TransactionKind> ShardedCursor<K> {
    pub(super) const fn new(cursors: Vec<Cursor<K>>, shift: u8) -> Self {
        Self { cursors, shift, current: None, deleted: false }
    }

    fn shard(&self, value: &[u8]) -> usize {
        if self.cursors.len() == 1 {
            0
        } else {
            usize::from(value[0] >> self.shift)
        }
    }

    fn select(&mut self, candidate: Option<(usize, Row)>) -> Pair<'static> {
        if let Some((i, row)) = candidate {
            self.deleted = false;
            self.current = Some((i, row.clone()));
            pair(Some(row))
        } else {
            // MDBX retains the current position on a failed movement.
            pair(None)
        }
    }

    fn extreme(&mut self, last: bool, key: Option<&[u8]>, exact: bool) -> Pair<'_> {
        if self.cursors.len() == 1 {
            let c = &mut self.cursors[0];
            return if let Some(key) = key {
                if exact {
                    c.set_key(key)
                } else {
                    c.set_range(key)
                }
            } else if last {
                c.last()
            } else {
                c.first()
            }
        }
        let mut best: Option<(usize, Row)> = None;
        for (i, cursor) in self.cursors.iter_mut().enumerate() {
            let row: Option<Row> = if let Some(key) = key {
                if exact {
                    cursor.set_key(key)?
                } else {
                    cursor.set_range(key)?
                }
            } else if last {
                cursor.last()?
            } else {
                cursor.first()?
            };
            if let Some(row) = row &&
                best.as_ref().is_none_or(|(_, b)| if last { row > *b } else { row < *b })
            {
                best = Some((i, row));
            }
        }
        self.select(best)
    }

    pub(super) fn first(&mut self) -> Pair<'_> {
        self.extreme(false, None, false)
    }
    pub(super) fn last(&mut self) -> Pair<'_> {
        self.extreme(true, None, false)
    }
    pub(super) fn set_key(&mut self, key: &[u8]) -> Pair<'_> {
        self.extreme(false, Some(key), true)
    }
    pub(super) fn set_range(&mut self, key: &[u8]) -> Pair<'_> {
        self.extreme(false, Some(key), false)
    }
    pub(super) fn set(&mut self, key: &[u8]) -> Value<'_> {
        Ok(self.set_key(key)?.map(|(_, v)| v))
    }
    pub(super) fn get_current(&mut self) -> Pair<'_> {
        if self.cursors.len() == 1 {
            return self.cursors[0].get_current();
        }
        if self.deleted {
            return pair(None)
        }
        pair(self.current.as_ref().map(|(_, row)| row.clone()))
    }

    pub(super) fn get_both_range(&mut self, key: &[u8], value: &[u8]) -> Value<'_> {
        if self.cursors.len() == 1 {
            return self.cursors[0].get_both_range(key, value)
        }
        let start = self.shard(value);
        for i in start..self.cursors.len() {
            if let Some(v) = self.cursors[i].get_both_range::<Vec<u8>>(key, value)? {
                self.select(Some((i, (key.to_vec(), v.clone()))))?;
                return Ok(Some(Cow::Owned(v)))
            }
        }
        Ok(None)
    }

    fn step(&mut self, reverse: bool, dup: bool, no_dup: bool) -> Pair<'_> {
        if self.cursors.len() == 1 {
            let c = &mut self.cursors[0];
            return if no_dup {
                c.next_nodup()
            } else if reverse {
                if dup {
                    c.prev_dup()
                } else {
                    c.prev()
                }
            } else if dup {
                c.next_dup()
            } else {
                c.next()
            };
        }
        let Some((_, (key, value))) = self.current.clone() else {
            return if reverse { self.last() } else { self.first() }
        };
        let mut best: Option<(usize, Row)> = None;
        for (i, c) in self.cursors.iter_mut().enumerate() {
            let mut row: Option<Row> = if no_dup {
                match c.set_range::<Vec<u8>, Vec<u8>>(&key)? {
                    Some((k, _)) if k == key => c.next_nodup()?,
                    other => other,
                }
            } else if let Some(v) = c.get_both_range::<Vec<u8>>(&key, &value)? {
                if reverse {
                    c.prev()?
                } else if v == value {
                    c.next()?
                } else {
                    Some((key.clone(), v))
                }
            } else if reverse {
                match c.set_range::<Vec<u8>, Vec<u8>>(&key)? {
                    Some((k, _)) if k == key => {
                        let v = c.last_dup::<Vec<u8>>()?.expect("positioned key");
                        Some((k, v))
                    }
                    Some(_) => c.prev()?,
                    None => c.last()?,
                }
            } else {
                match c.set_range::<Vec<u8>, Vec<u8>>(&key)? {
                    Some((k, _)) if k == key => c.next_nodup()?,
                    other => other,
                }
            };
            if dup && row.as_ref().is_some_and(|(k, _)| *k != key) {
                row = None;
            }
            if let Some(row) = row &&
                best.as_ref().is_none_or(|(_, b)| if reverse { row > *b } else { row < *b })
            {
                best = Some((i, row));
            }
        }
        self.select(best)
    }

    pub(super) fn next(&mut self) -> Pair<'_> {
        self.step(false, false, false)
    }
    pub(super) fn prev(&mut self) -> Pair<'_> {
        self.step(true, false, false)
    }
    pub(super) fn next_dup(&mut self) -> Pair<'_> {
        self.step(false, true, false)
    }
    pub(super) fn prev_dup(&mut self) -> Pair<'_> {
        self.step(true, true, false)
    }
    pub(super) fn next_nodup(&mut self) -> Pair<'_> {
        self.step(false, false, true)
    }
    pub(super) fn last_dup(&mut self) -> Value<'_> {
        if self.cursors.len() == 1 {
            return self.cursors[0].last_dup()
        }
        let Some((_, (key, _))) = self.current.clone() else { return Ok(None) };
        for i in (0..self.cursors.len()).rev() {
            if self.cursors[i].set::<Vec<u8>>(&key)?.is_some() {
                let value = self.cursors[i].last_dup::<Vec<u8>>()?.expect("positioned key");
                self.select(Some((i, (key, value.clone()))))?;
                return Ok(Some(Cow::Owned(value)))
            }
        }
        Ok(None)
    }
}

impl ShardedCursor<RW> {
    pub(super) fn put(&mut self, key: &[u8], value: &[u8], flags: WriteFlags) -> Result<()> {
        if self.cursors.len() == 1 {
            return self.cursors[0].put(key, value, flags)
        }
        if flags.contains(WriteFlags::NO_OVERWRITE) && self.set_key(key)?.is_some() {
            return Err(Error::KeyExist)
        }
        if flags.contains(WriteFlags::APPEND) && self.last()?.is_some_and(|(k, _)| key < k.as_ref())
        {
            return Err(Error::KeyExist)
        }
        if flags.contains(WriteFlags::APPEND_DUP) &&
            self.set_key(key)?.is_some() &&
            self.last_dup()?.is_some_and(|v| value <= v.as_ref())
        {
            return Err(Error::KeyExist)
        }
        let i = self.shard(value);
        self.cursors[i].put(key, value, flags)?;
        self.current = Some((i, (key.to_vec(), value.to_vec())));
        self.deleted = false;
        Ok(())
    }

    pub(super) fn del(&mut self, flags: WriteFlags) -> Result<()> {
        if self.cursors.len() == 1 {
            return self.cursors[0].del(flags)
        }
        let Some((i, (key, value))) = self.current.clone() else { return Err(Error::NotFound) };
        if flags.contains(WriteFlags::NO_DUP_DATA) {
            for c in &mut self.cursors {
                if c.set::<Vec<u8>>(&key)?.is_some() {
                    c.del(WriteFlags::NO_DUP_DATA)?;
                }
            }
        } else {
            self.cursors[i].get_both::<Vec<u8>>(&key, &value)?.ok_or(Error::NotFound)?;
            self.cursors[i].del(flags)?;
        }
        self.deleted = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::migrate_storage_shards;
    use crate::{init_db, mdbx::DatabaseArguments, open_db, tables::HashedStorages, Database};
    use alloy_primitives::{B256, U256};
    use reth_db_api::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::StorageEntry;

    fn entry(prefix: u8) -> StorageEntry {
        StorageEntry { key: B256::repeat_byte(prefix), value: U256::from(prefix as u64 + 1) }
    }

    #[test]
    fn prefix_shards_preserve_cursor_order_deletion_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = init_db(dir.path(), DatabaseArguments::test()).unwrap();
        let tx = db.tx_mut().unwrap();
        let mut expected = Vec::new();
        for address in [1, 3, 5] {
            for prefix in [0, 31, 64, 95, 128, 159, 192, 255] {
                let row = (B256::repeat_byte(address), entry(prefix));
                tx.put::<HashedStorages>(row.0, row.1).unwrap();
                expected.push(row);
            }
        }
        assert_eq!(tx.entries::<HashedStorages>().unwrap(), expected.len());
        let mut c = tx.cursor_dup_write::<HashedStorages>().unwrap();
        assert_eq!(c.walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap(), expected);
        assert_eq!(
            c.walk_back(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
            expected.iter().rev().copied().collect::<Vec<_>>()
        );
        for prefix in 0..=255 {
            let expected_entry = expected
                .iter()
                .find(|(k, v)| *k == B256::repeat_byte(3) && v.key >= B256::repeat_byte(prefix));
            assert_eq!(
                c.seek_by_key_subkey(B256::repeat_byte(3), B256::repeat_byte(prefix)).unwrap(),
                expected_entry.map(|(_, v)| *v)
            );
        }
        for (index, row) in expected.iter().enumerate() {
            assert_eq!(c.seek_by_key_subkey(row.0, row.1.key).unwrap(), Some(row.1));
            assert_eq!(c.current().unwrap(), Some(*row));
            assert_eq!(c.next().unwrap(), expected.get(index + 1).copied());
            c.seek_by_key_subkey(row.0, row.1.key).unwrap();
            assert_eq!(c.prev().unwrap(), index.checked_sub(1).map(|i| expected[i]));
        }
        let address = B256::repeat_byte(3);
        c.seek_exact(address).unwrap();
        assert_eq!(c.last_dup().unwrap(), Some(entry(255)));
        assert_eq!(c.prev_dup().unwrap(), Some((address, entry(192))));
        c.seek_by_key_subkey(address, entry(95).key).unwrap();
        c.delete_current().unwrap();
        assert_eq!(c.next_dup().unwrap(), Some((address, entry(128))));
        c.delete_current_duplicates().unwrap();
        assert_eq!(c.next().unwrap(), Some((B256::repeat_byte(5), entry(0))));
        expected.retain(|(k, _)| *k != address);
        assert_eq!(c.walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap(), expected);
        drop(c);
        tx.commit().unwrap();
        drop(db);
        let db = open_db(dir.path(), DatabaseArguments::test()).unwrap();
        let tx = db.tx().unwrap();
        assert_eq!(
            tx.cursor_read::<HashedStorages>()
                .unwrap()
                .walk(None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            expected
        );
        drop(tx);
        let tx = db.tx_mut().unwrap();
        assert!(tx.delete::<HashedStorages>(B256::repeat_byte(5), None).unwrap());
        assert_eq!(tx.entries::<HashedStorages>().unwrap(), 8);
        tx.clear::<HashedStorages>().unwrap();
        assert_eq!(tx.entries::<HashedStorages>().unwrap(), 0);
        tx.abort();
        assert_eq!(db.tx().unwrap().entries::<HashedStorages>().unwrap(), 16);
    }

    #[test]
    fn all_shard_children_abort_with_parent() {
        let dir = tempfile::tempdir().unwrap();
        let db = init_db(dir.path(), DatabaseArguments::test()).unwrap();
        for commit in [false, true] {
            let tx = db.tx_mut().unwrap();
            tx.enable_parallel_writes_for_tables_with_hints(&[("HashedStorages", 4000)]).unwrap();
            let cursors = tx.cursor_dup_write_shards::<HashedStorages>().unwrap();
            assert_eq!(cursors.len(), 4);
            std::thread::scope(|scope| {
                for (i, mut c) in cursors.into_iter().enumerate() {
                    scope.spawn(move || c.upsert(B256::ZERO, &entry((i * 64) as u8)).unwrap());
                }
            });
            tx.commit_subtxns().unwrap();
            assert_eq!(tx.entries::<HashedStorages>().unwrap(), 4);
            if commit {
                tx.commit().unwrap();
            } else {
                tx.abort();
            }
            assert_eq!(
                db.tx().unwrap().entries::<HashedStorages>().unwrap(),
                if commit { 4 } else { 0 }
            );
        }
    }

    #[test]
    fn migrate_legacy_and_packed_snapshots_and_resume_publication() {
        use crate::version::{db_version_file_path, get_db_version};
        use reth_db_api::{
            models::StorageSettings,
            tables::{Metadata, RawTable, StoragesTrie},
        };
        use reth_libmdbx::WriteFlags;
        for packed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let db = init_db(dir.path(), DatabaseArguments::test()).unwrap();
            let tx = db.tx_mut().unwrap();
            tx.put::<Metadata>(
                "storage_settings".to_owned(),
                serde_json::to_vec(&StorageSettings { storage_v2: packed }).unwrap(),
            )
            .unwrap();
            tx.commit().unwrap();
            let tx = db.inner.begin_rw_txn().unwrap();
            let hashed = tx.open_db(Some("HashedStorages")).unwrap();
            let trie = tx.open_db(Some("StoragesTrie")).unwrap();
            for address in 0..4u8 {
                for prefix in 0..16u8 {
                    use reth_db_api::table::Compress;
                    let value = entry(prefix * 16).compress();
                    tx.put(hashed.dbi(), [address; 32], value, WriteFlags::UPSERT).unwrap();
                    let mut value = vec![0; if packed { 34 } else { 66 }];
                    value[0] = if packed { prefix << 4 } else { prefix };
                    tx.put(trie.dbi(), [address; 32], value, WriteFlags::UPSERT).unwrap();
                }
            }
            tx.commit().unwrap();
            drop((hashed, trie));
            drop(db);
            // Simulate both the ordinary v2 entry point and an interruption before
            // committing the conversion transaction (quarantined, untouched data).
            reth_fs_util::write(
                db_version_file_path(dir.path()),
                if packed { "3000003" } else { "2" },
            )
            .unwrap();
            migrate_storage_shards(dir.path()).unwrap();
            assert_eq!(get_db_version(dir.path()).unwrap(), 3);
            let db = open_db(dir.path(), DatabaseArguments::test()).unwrap();
            let tx = db.tx().unwrap();
            assert_eq!(tx.entries::<HashedStorages>().unwrap(), 64);
            assert_eq!(tx.entries::<StoragesTrie>().unwrap(), 64);
            let rows = tx
                .cursor_read::<HashedStorages>()
                .unwrap()
                .walk(None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            for (i, (key, value)) in rows.iter().enumerate() {
                assert_eq!(*key, B256::repeat_byte((i / 16) as u8));
                assert_eq!(*value, entry(((i % 16) * 16) as u8));
            }
            assert_eq!(
                tx.cursor_read::<RawTable<StoragesTrie>>().unwrap().walk(None).unwrap().count(),
                64
            );
            for name in super::HASHED.iter().chain(super::TRIE.iter()) {
                assert_eq!(
                    tx.inner().db_stat(tx.get_dbi_raw(name).unwrap()).unwrap().entries(),
                    16
                );
            }
            drop(tx);
            drop(db);
            // Simulate a crash after MDBX commit, before publishing version 3.
            reth_fs_util::write(db_version_file_path(dir.path()), "3000003").unwrap();
            migrate_storage_shards(dir.path()).unwrap();
            migrate_storage_shards(dir.path()).unwrap();
            assert_eq!(get_db_version(dir.path()).unwrap(), 3);
        }
    }
}
