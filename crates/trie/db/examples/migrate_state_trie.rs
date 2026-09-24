//! Migrate a fully persisted snapshot with the `migrate_state_trie` example.
//! See `docs/state-trie-db.md` for usage and the supported execution path.
use alloy_primitives::{B256, U256};
use reth_db::{mdbx::DatabaseArguments, Database, DatabaseEnv, DatabaseEnvKind};
use reth_db_api::{
    cursor::DbCursorRO,
    models::StorageSettings,
    tables,
    transaction::{DbTx, DbTxMut},
};
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

fn verify_persisted_root(db: &DatabaseEnv, expected: B256) -> Result<()> {
    let tx = db.tx()?;
    let mut calculator = proof_v3::ProofCalculator::new(
        DatabaseStateTrieCursorFactory(&tx).state_trie_account_cursor()?,
    );
    let root = calculator.root_node()?;
    let actual = calculator.compute_root_hash(&[root])?.expect("root");
    assert_eq!(actual, expected, "persisted root must match the source trie");
    println!("persisted_state_root={actual}");
    Ok(())
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
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: migrate_state_trie DATADIR/db [--check | --restart]")?;
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
    if std::env::args().any(|s| s == "--check") {
        if db.tx()?.get::<tables::Metadata>("state_trie_db_migration".into())?.is_some() {
            verify_persisted_root(&db, expected)?;
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
    if std::env::args().any(|s| s == "--restart") {
        let tx = db.tx_mut()?;
        assert!(
            tx.get::<tables::Metadata>("state_trie_db_migration".into())?.is_none(),
            "refusing to restart a completed migration"
        );
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
    drop(tx);
    let mut writer = Writer {
        db: &db,
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
    verify_persisted_root(&db, expected)?;
    let tx = db.tx_mut()?;
    tx.put::<tables::Metadata>(
        "state_trie_db_migration".into(),
        format!("{}:{actual}", checkpoint.block_number).into_bytes(),
    )?;
    tx.commit()?;
    let (old, new) = sizes(&db)?;
    println!("source_bytes={old} state_trie_bytes={new} difference_bytes={} ratio={:.6} state_root={actual}", new as i128-old as i128, new as f64 / old as f64);
    Ok(())
}
