//! Copy legacy hashed leaves and compact branch nodes from MDBX to `RocksDB`.
//! Usage: `migrate_legacy_trie DATADIR/db ROCKSDB_PATH [--check | --check-root]`
use alloy_primitives::B256;
use reth_db::{mdbx::DatabaseArguments, Database};
use reth_db_api::{
    cursor::DbCursorRO,
    models::{state_trie::StateTrieStorageKey, CompactU256, StorageSettings},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_provider::providers::{legacy_storage_key, RocksDBProvider};
use reth_trie::{hashed_cursor::HashedCursorFactory, proof_v2, trie_cursor::TrieCursorFactory};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    StorageTrieEntryLike, TrieTableAdapter,
};
use std::{error::Error, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
const MARKER: &str = "legacy_trie_rocksdb_migration";

fn root(trie: impl TrieCursorFactory, hashed: impl HashedCursorFactory) -> Result<B256> {
    let mut calc = proof_v2::ProofCalculator::new(
        trie.account_trie_cursor()?,
        hashed.hashed_account_cursor()?,
    );
    let mut encoder = proof_v2::SyncAccountValueEncoder::new(&trie, &hashed);
    let node = calc.root_node(&mut encoder)?;
    Ok(calc.compute_root_hash(&[node])?.ok_or("missing root")?)
}

fn copy_branches<A: TrieTableAdapter>(
    db: &reth_db::DatabaseEnv,
    rocks: &RocksDBProvider,
    work: &Path,
    storage: bool,
) -> Result<(u64, B256)> {
    let mut tx = db.tx()?;
    tx.disable_long_read_transaction_safety();
    if storage {
        let mut cursor = tx.cursor_read::<A::StorageTrieTable>()?;
        let rows = cursor.walk(None)?.map(|entry| {
            let (address, entry) = entry?;
            let (path, node) = entry.into_parts();
            Ok((StateTrieStorageKey { address, path: A::subkey_to_nibbles(&path).into() }, node))
        });
        Ok(rocks.import_legacy_table::<tables::RocksStoragesTrie>(rows, work)?)
    } else {
        let mut cursor = tx.cursor_read::<A::AccountTrieTable>()?;
        let rows = cursor.walk(None)?.map(|entry| {
            let (path, node) = entry?;
            Ok((A::account_key_to_nibbles(&path).into(), node))
        });
        Ok(rocks.import_legacy_table::<tables::RocksAccountsTrie>(rows, work)?)
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let db_path = args.next().ok_or("expected MDBX path")?;
    let rocks_path = args.next().ok_or("expected RocksDB path")?;
    let mode = args.next();
    let check = match mode.as_deref() {
        None => false,
        Some("--check" | "--check-root") => true,
        _ => return Err("unknown argument".into()),
    };
    if args.next().is_some() {
        return Err("too many arguments".into())
    }
    // Assert the frontier before creating files, tables, or any writable database handle.
    let db = reth_db::open_db_read_only(&db_path, DatabaseArguments::default())?;
    let tx = db.tx()?;
    let finish = tx.get::<tables::StageCheckpoints>("Finish".into())?.ok_or("missing Finish")?;
    let partial = finish
        .finish_stage_checkpoint()
        .and_then(|c| c.partial_state_trie())
        .unwrap_or(finish.block_number);
    assert_eq!(
        partial, finish.block_number,
        "partial_state_trie must equal Finish before migration"
    );
    let marker = tx.get::<tables::Metadata>(MARKER.into())?;
    if !check && marker.is_some() {
        return Err("legacy migration already completed".into())
    }
    let settings: StorageSettings = tx
        .get::<tables::Metadata>("storage_settings".into())?
        .map(|v| serde_json::from_slice(&v))
        .transpose()?
        .unwrap_or_default();
    let expected = if settings.is_v2() {
        root(
            DatabaseTrieCursorFactory::<_, PackedKeyAdapter>::new(&tx),
            DatabaseHashedCursorFactory::new(&tx),
        )?
    } else {
        root(
            DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx),
            DatabaseHashedCursorFactory::new(&tx),
        )?
    };
    println!(
        "finish={} partial_state_trie={partial} expected_state_root={expected}",
        finish.block_number
    );
    drop(tx);
    let rocks = RocksDBProvider::builder(&rocks_path)
        .with_default_tables()
        .with_table::<tables::HashedAccounts>()
        .with_table::<tables::RocksHashedStorages>()
        .with_table::<tables::RocksAccountsTrie>()
        .with_table::<tables::RocksStoragesTrie>()
        .with_read_only(check)
        .build()?;
    let records = if check {
        let saved: (u64, B256, Vec<(u64, B256)>) =
            serde_json::from_slice(&marker.ok_or("missing migration marker")?)?;
        assert_eq!(
            (saved.0, saved.1),
            (finish.block_number, expected),
            "snapshot changed since migration"
        );
        let actual = if mode.as_deref() == Some("--check-root") {
            saved.2.clone()
        } else {
            vec![
                rocks.legacy_table_digest::<tables::HashedAccounts>()?,
                rocks.legacy_table_digest::<tables::RocksHashedStorages>()?,
                rocks.legacy_table_digest::<tables::RocksAccountsTrie>()?,
                rocks.legacy_table_digest::<tables::RocksStoragesTrie>()?,
            ]
        };
        assert_eq!(actual, saved.2, "legacy migration table digest mismatch");
        actual
    } else {
        // Independent tables use independent MDBX readers and bounded SST writers.
        let work = Path::new(&rocks_path).join("legacy-import");
        std::thread::scope(|scope| -> Result<Vec<(u64, B256)>> {
            let accounts = scope.spawn(|| -> Result<_> {
                let mut tx = db.tx()?;
                tx.disable_long_read_transaction_safety();
                let mut cursor = tx.cursor_read::<tables::HashedAccounts>()?;
                Ok(rocks
                    .import_legacy_table::<tables::HashedAccounts>(cursor.walk(None)?, &work)?)
            });
            let storage = scope.spawn(|| -> Result<_> {
                let mut tx = db.tx()?;
                tx.disable_long_read_transaction_safety();
                let mut cursor = tx.cursor_read::<tables::HashedStorages>()?;
                let rows = cursor.walk(None)?.map(|entry| {
                    let (address, entry) = entry?;
                    Ok((legacy_storage_key(address, entry.key), CompactU256(entry.value)))
                });
                Ok(rocks.import_legacy_table::<tables::RocksHashedStorages>(rows, &work)?)
            });
            let account_trie = scope.spawn(|| {
                if settings.is_v2() {
                    copy_branches::<PackedKeyAdapter>(&db, &rocks, &work, false)
                } else {
                    copy_branches::<LegacyKeyAdapter>(&db, &rocks, &work, false)
                }
            });
            let storage_trie = scope.spawn(|| {
                if settings.is_v2() {
                    copy_branches::<PackedKeyAdapter>(&db, &rocks, &work, true)
                } else {
                    copy_branches::<LegacyKeyAdapter>(&db, &rocks, &work, true)
                }
            });
            let results =
                [accounts.join(), storage.join(), account_trie.join(), storage_trie.join()];
            results.into_iter().map(|r| r.map_err(|_| "migration worker panicked")?).collect()
        })?
    };
    let snapshot = rocks.snapshot();
    let actual = root(&snapshot, &snapshot)?;
    assert_eq!(actual, expected, "RocksDB legacy root mismatch");
    println!("persisted_legacy_state_root={actual} records_and_digests={records:?}");
    drop(snapshot);
    drop(rocks);
    drop(db);
    if !check {
        let db = reth_db::open_db(&db_path, DatabaseArguments::default())?;
        let tx = db.tx_mut()?;
        tx.put::<tables::Metadata>(
            MARKER.into(),
            serde_json::to_vec(&(finish.block_number, actual, records))?,
        )?;
        tx.commit()?;
    }
    Ok(())
}
