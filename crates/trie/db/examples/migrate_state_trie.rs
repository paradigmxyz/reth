//! Migrate a fully persisted snapshot with the `migrate_state_trie` example.
use alloy_primitives::{B256, U256};
use reth_db::{mdbx::DatabaseArguments, Database, DatabaseEnv, DatabaseEnvKind};
use reth_db_api::{
    cursor::DbCursorRO,
    models::{state_trie::StateTrieStorageKey, StorageSettings},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_provider::providers::RocksDBProvider;
use reth_trie::{
    hashed_cursor::HashedCursorFactory, proof_v2, proof_v3,
    state_trie_cursor::StateTrieCursorFactory, trie_cursor::TrieCursorFactory, Nibbles,
    StateTrieBuilder, StateTrieNode, StateTrieStorageEntry, TrieAccount,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateTrieCursorFactory, DatabaseTrieCursorFactory,
    LegacyKeyAdapter, PackedKeyAdapter, TrieTableAdapter,
};
use std::{error::Error, io::Write, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn old_root<A: TrieTableAdapter>(tx: &impl DbTx) -> Result<B256> {
    let trie = DatabaseTrieCursorFactory::<_, A>::new(tx);
    let hashed = DatabaseHashedCursorFactory::new(tx);
    let mut calc = proof_v2::ProofCalculator::new(
        trie.account_trie_cursor()?,
        hashed.hashed_account_cursor()?,
    );
    let mut encoder = proof_v2::SyncAccountValueEncoder::new(trie, hashed);
    let root = calc.root_node(&mut encoder)?;
    Ok(calc.compute_root_hash(&[root])?.expect("root"))
}

fn verify_persisted_root(factory: impl StateTrieCursorFactory, expected: B256) -> Result<()> {
    let mut calculator = proof_v3::ProofCalculator::new(factory.state_trie_account_cursor()?);
    let root = calculator.root_node()?;
    let actual = calculator.compute_root_hash(&[root])?.expect("root");
    assert_eq!(actual, expected, "persisted root must match the source trie");
    println!("persisted_state_root={actual}");
    Ok(())
}

fn rocks_sizes(rocksdb: &RocksDBProvider) {
    for stat in rocksdb.table_stats().into_iter().filter(|s| s.name.starts_with("StateTrie")) {
        println!("rocksdb_table={} estimated_entries={} sst_bytes={} memtable_bytes={} pending_compaction_bytes={}",
            stat.name, stat.estimated_num_keys, stat.sst_size_bytes, stat.memtable_size_bytes, stat.pending_compaction_bytes);
    }
}

fn sizes(db: &DatabaseEnv) -> Result<(usize, usize)> {
    let tx = db.tx()?;
    let mut totals = [0, 0];
    for (group, tables) in [
        ["HashedAccounts", "HashedStorages", "AccountsTrie", "StoragesTrie"].as_slice(),
        ["StateTrieAccounts", "StateTrieStorages"].as_slice(),
    ]
    .iter()
    .enumerate()
    {
        for name in *tables {
            let Ok(table) = tx.inner().open_db(Some(name)) else { continue };
            let stat = tx.inner().db_stat(table.dbi())?;
            let bytes = stat.page_size() as usize *
                (stat.leaf_pages() + stat.branch_pages() + stat.overflow_pages());
            totals[group] += bytes;
            println!("table={name} entries={} allocated_bytes={bytes}", stat.entries());
        }
    }
    Ok(totals.into())
}

struct Writer<'a> {
    db: &'a DatabaseEnv,
    rocksdb: Option<&'a RocksDBProvider>,
    accounts: Vec<(Nibbles, StateTrieNode<TrieAccount>)>,
    storage: Vec<(B256, Nibbles, StateTrieNode<U256>)>,
    count: usize,
    started: Instant,
}
impl Writer<'_> {
    fn flush(&mut self) -> Result<()> {
        let count = self.accounts.len() + self.storage.len();
        if count == 0 {
            return Ok(())
        }
        if let Some(rocksdb) = self.rocksdb {
            let mut batch = rocksdb.batch();
            for (path, node) in self.accounts.drain(..) {
                batch.put::<tables::StateTrieAccounts>(path.into(), &node)?;
            }
            for (address, path, node) in self.storage.drain(..) {
                batch.put::<tables::RocksStateTrieStorages>(
                    StateTrieStorageKey { address, path: path.into() },
                    &node,
                )?;
            }
            batch.commit()?;
        } else {
            let tx = self.db.tx_mut()?;
            for (path, node) in self.accounts.drain(..) {
                tx.put::<tables::StateTrieAccounts>(path.into(), node)?;
            }
            for (address, path, node) in self.storage.drain(..) {
                tx.put::<tables::StateTrieStorages>(
                    address,
                    StateTrieStorageEntry { nibbles: path.into(), node },
                )?;
            }
            tx.commit()?;
        }
        self.count += count;
        let _ = writeln!(
            std::io::stderr(),
            "nodes={} elapsed_secs={:.1}",
            self.count,
            self.started.elapsed().as_secs_f64()
        );
        Ok(())
    }
    fn maybe_flush(&mut self) -> Result<()> {
        if self.accounts.len() + self.storage.len() >= 200_000 {
            self.flush()?;
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    migrate(std::env::args().skip(1))
}

fn migrate(mut args: impl Iterator<Item = String>) -> Result<()> {
    let path = args.next().ok_or(
        "usage: migrate_state_trie DATADIR/db [--rocksdb PATH] [--check | --restart | --rewrite]",
    )?;
    let (mut check, mut restart, mut rewrite, mut rocks_path) = (false, false, false, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--restart" => restart = true,
            "--rewrite" => rewrite = true,
            "--rocksdb" => {
                rocks_path = Some(args.next().ok_or("--rocksdb requires a destination path")?)
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if u8::from(check) + u8::from(restart) + u8::from(rewrite) > 1 {
        return Err("--check, --restart and --rewrite are mutually exclusive".into())
    }
    if rewrite && rocks_path.is_none() {
        return Err("--rewrite requires --rocksdb".into())
    }
    let marker = if rocks_path.is_some() {
        "state_trie_rocksdb_migration"
    } else {
        "state_trie_db_migration"
    };
    // Read and assert the frontier before opening a write environment or creating any tables.
    let db = reth_db::open_db_read_only(&path, DatabaseArguments::default())?;
    let tx = db.tx()?;
    let checkpoint =
        tx.get::<tables::StageCheckpoints>("Finish".into())?.ok_or("missing Finish checkpoint")?;
    let partial = checkpoint
        .finish_stage_checkpoint()
        .and_then(|c| c.partial_state_trie())
        .unwrap_or(checkpoint.block_number);
    assert_eq!(
        partial, checkpoint.block_number,
        "partial_state_trie must equal Finish before migration"
    );
    println!("finish={} partial_state_trie={partial}", checkpoint.block_number);
    let settings: StorageSettings = tx
        .get::<tables::Metadata>("storage_settings".into())?
        .map(|b| serde_json::from_slice(&b))
        .transpose()?
        .unwrap_or_default();
    let expected = if settings.is_v2() {
        old_root::<PackedKeyAdapter>(&tx)?
    } else {
        old_root::<LegacyKeyAdapter>(&tx)?
    };
    println!("expected_state_root={expected}");
    drop(tx);
    sizes(&db)?;
    let rocksdb = rocks_path
        .as_ref()
        .map(|path| {
            RocksDBProvider::builder(path)
                .with_default_tables()
                .with_read_only(check)
                .with_max_subcompactions(if rewrite { 16 } else { 1 })
                .build()
        })
        .transpose()?;
    if rewrite {
        let rocksdb = rocksdb.as_ref().expect("--rewrite requires --rocksdb");
        verify_persisted_root(rocksdb.snapshot(), expected)?;
        rocks_sizes(rocksdb);
        println!("rewriting state trie SST files");
        rocksdb.flush_and_compact_tables(&["StateTrieAccounts", "StateTrieStorages"], true)?;
        verify_persisted_root(rocksdb.snapshot(), expected)?;
        rocks_sizes(rocksdb);
        return Ok(())
    }
    if check {
        if let Some(rocksdb) = &rocksdb {
            verify_persisted_root(rocksdb.snapshot(), expected)?;
            rocks_sizes(rocksdb);
        } else if db.tx()?.get::<tables::Metadata>(marker.into())?.is_some() {
            verify_persisted_root(DatabaseStateTrieCursorFactory(&db.tx()?), expected)?;
        }
        return Ok(())
    }
    drop(db);
    let mut db = DatabaseEnv::open(
        std::path::Path::new(&path),
        DatabaseEnvKind::RW,
        DatabaseArguments::default(),
    )?;
    db.create_tables()?;
    assert!(
        db.tx()?.get::<tables::Metadata>(marker.into())?.is_none(),
        "destination migration already completed"
    );
    if let Some(rocksdb) = &rocksdb {
        if restart {
            rocksdb.clear::<tables::StateTrieAccounts>()?;
            rocksdb.clear::<tables::RocksStateTrieStorages>()?;
        }
        assert!(
            rocksdb.first::<tables::StateTrieAccounts>()?.is_none(),
            "destination account table must be empty"
        );
        assert!(
            rocksdb.first::<tables::RocksStateTrieStorages>()?.is_none(),
            "destination storage table must be empty"
        );
    } else {
        if restart {
            let tx = db.tx_mut()?;
            tx.clear::<tables::StateTrieAccounts>()?;
            tx.clear::<tables::StateTrieStorages>()?;
            tx.commit()?;
        }
        let tx = db.tx()?;
        assert_eq!(
            tx.entries::<tables::StateTrieAccounts>()?,
            0,
            "destination account table must be empty"
        );
        assert_eq!(
            tx.entries::<tables::StateTrieStorages>()?,
            0,
            "destination storage table must be empty"
        );
    }
    let mut writer = Writer {
        db: &db,
        rocksdb: rocksdb.as_ref(),
        accounts: Vec::new(),
        storage: Vec::new(),
        count: 0,
        started: Instant::now(),
    };
    let mut account_builder = StateTrieBuilder::default();
    let mut next_account = Some(B256::ZERO);
    let mut storage_leaves = 0usize;
    while let Some(start) = next_account {
        let mut tx = db.tx()?;
        tx.disable_long_read_transaction_safety();
        let mut accounts = tx.cursor_read::<tables::HashedAccounts>()?;
        let mut storage = tx.cursor_read::<tables::HashedStorages>()?;
        let mut slot = storage.seek(start)?;
        let mut row = accounts.seek(start)?;
        for _ in 0..25_000 {
            let Some((address, account)) = row.take() else { break };
            let mut storage_builder = StateTrieBuilder::default();
            let mut write_storage = |path, node| {
                writer.storage.push((address, path, node));
                writer.maybe_flush()
            };
            while slot.as_ref().is_some_and(|(a, _)| *a == address) {
                let (_, entry) = slot.take().expect("storage entry");
                assert!(!entry.value.is_zero(), "zero hashed storage leaf");
                storage_builder.push(entry.key, entry.value, &mut write_storage)?;
                storage_leaves += 1;
                slot = storage.next()?;
            }
            assert!(slot.as_ref().is_none_or(|(a, _)| *a > address), "storage without an account");
            let root = storage_builder.finish(&mut write_storage)?;
            account_builder.push(address, account.into_trie_account(root), &mut |path, node| {
                writer.accounts.push((path, node));
                writer.maybe_flush()
            })?;
            row = accounts.next()?;
        }
        next_account = row.map(|(a, _)| a);
    }
    assert_eq!(
        storage_leaves,
        db.tx()?.entries::<tables::HashedStorages>()?,
        "all storage leaves must be migrated"
    );
    let actual = account_builder.finish(&mut |path, node| {
        writer.accounts.push((path, node));
        writer.maybe_flush()
    })?;
    assert_eq!(actual, expected, "migrated root must match the source trie");
    writer.flush()?;
    if let Some(rocksdb) = &rocksdb {
        rocksdb.flush_and_compact_tables(&["StateTrieAccounts", "StateTrieStorages"], false)?;
        verify_persisted_root(rocksdb.snapshot(), expected)?;
        rocks_sizes(rocksdb);
    } else {
        verify_persisted_root(DatabaseStateTrieCursorFactory(&db.tx()?), expected)?;
    }
    let tx = db.tx_mut()?;
    tx.put::<tables::Metadata>(
        marker.into(),
        format!("{}:{actual}", checkpoint.block_number).into_bytes(),
    )?;
    tx.commit()?;
    let (old, new) = sizes(&db)?;
    if rocksdb.is_none() {
        println!("source_bytes={old} state_trie_bytes={new} difference_bytes={} ratio={:.6} state_root={actual}", new as i128-old as i128, new as f64 / old as f64);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint};

    #[test]
    fn migration_preserves_source_and_populates_both_backends() {
        let (dir, _) = reth_db::test_utils::create_test_rocksdb_dir();
        let path = dir.path().join("db");
        let rocks = dir.path().join("rocksdb");
        let db = reth_db::init_db(&path, DatabaseArguments::test()).unwrap();
        let tx = db.tx_mut().unwrap();
        tx.put::<tables::StageCheckpoints>("Finish".into(), StageCheckpoint::new(7)).unwrap();
        for i in 1..5 {
            let address = B256::with_last_byte(i);
            tx.put::<tables::HashedAccounts>(
                address,
                Account { nonce: i as u64, ..Default::default() },
            )
            .unwrap();
            for j in 1..4 {
                tx.put::<tables::HashedStorages>(
                    address,
                    StorageEntry { key: B256::with_last_byte(j), value: U256::from(j) },
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
        drop(db);
        let path = path.to_str().unwrap().to_owned();
        let rocks = rocks.to_str().unwrap().to_owned();
        migrate([path.clone()].into_iter()).unwrap();
        migrate([path.clone(), "--rocksdb".into(), rocks.clone()].into_iter()).unwrap();
        migrate([path.clone(), "--check".into()].into_iter()).unwrap();
        migrate([path.clone(), "--rocksdb".into(), rocks.clone(), "--check".into()].into_iter())
            .unwrap();
        migrate([path.clone(), "--rocksdb".into(), rocks.clone(), "--rewrite".into()].into_iter())
            .unwrap();
        let db = reth_db::open_db_read_only(&path, DatabaseArguments::test()).unwrap();
        let tx = db.tx().unwrap();
        assert_eq!(tx.entries::<tables::HashedAccounts>().unwrap(), 4);
        assert_eq!(tx.entries::<tables::HashedStorages>().unwrap(), 12);
        let rocks = RocksDBProvider::builder(rocks).with_default_tables().build().unwrap();
        for row in tx.cursor_read::<tables::StateTrieAccounts>().unwrap().walk(None).unwrap() {
            let (key, node) = row.unwrap();
            assert_eq!(rocks.get::<tables::StateTrieAccounts>(key).unwrap(), Some(node));
        }
        for row in tx.cursor_read::<tables::StateTrieStorages>().unwrap().walk(None).unwrap() {
            let (address, entry) = row.unwrap();
            assert_eq!(
                rocks
                    .get::<tables::RocksStateTrieStorages>(StateTrieStorageKey {
                        address,
                        path: entry.nibbles.0.into()
                    })
                    .unwrap(),
                Some(entry.node)
            );
        }
    }

    #[test]
    fn migration_rejects_partial_state_before_opening_rocksdb() {
        let (dir, _) = reth_db::test_utils::create_test_rocksdb_dir();
        let path = dir.path().join("db");
        let rocks = dir.path().join("rocksdb");
        let db = reth_db::init_db(&path, DatabaseArguments::test()).unwrap();
        let tx = db.tx_mut().unwrap();
        tx.put::<tables::StageCheckpoints>(
            "Finish".into(),
            StageCheckpoint::new(7)
                .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(6) }),
        )
        .unwrap();
        tx.commit().unwrap();
        drop(db);
        assert!(std::panic::catch_unwind(|| migrate(
            [path.to_str().unwrap().into(), "--rocksdb".into(), rocks.to_str().unwrap().into()]
                .into_iter()
        ))
        .is_err());
        assert!(!rocks.exists());
    }
}
