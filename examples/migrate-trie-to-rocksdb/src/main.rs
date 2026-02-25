#![allow(unused_crate_dependencies)]

//! Copies trie data from MDBX to RocksDB (one-time migration for storage v2).
//!
//! Assumes `migrate-trie-to-packed` has already been run so that MDBX trie tables
//! use packed nibble encoding. Reads `PackedAccountsTrie` and `PackedStoragesTrie`
//! from MDBX and writes them into the RocksDB column families that the node expects
//! when running with `--storage.v2`.
//!
//! By default the MDBX trie tables are left intact. Pass `--clear-mdbx` to remove
//! them after a successful copy (reclaim ~2 GiB of MDBX space).
//!
//! **Back up your datadir before running this.**
//!
//! Usage:
//! ```sh
//! cargo run -p example-migrate-trie-to-rocksdb -- --datadir ~/.local/share/reth/mainnet
//! cargo run -p example-migrate-trie-to-rocksdb -- --datadir ~/.local/share/reth/mainnet --clear-mdbx
//! ```

use clap::Parser;
use eyre::Result;
use reth_db::{mdbx::DatabaseArguments, open_db};
use reth_db_api::{
    cursor::DbCursorRO,
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_provider::providers::{RocksDBBuilder, RocksDBProvider};
use reth_trie_common::PackedStoredNibblesSubKey;
use reth_trie_db::{PackedAccountsTrie, PackedStoragesTrie};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Copy trie data from MDBX to RocksDB for storage v2")]
struct Cli {
    /// Path to the reth datadir (e.g. ~/.local/share/reth/mainnet)
    #[arg(long)]
    datadir: PathBuf,

    /// Remove trie tables from MDBX after successful copy
    #[arg(long)]
    clear_mdbx: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let db_path = cli.datadir.join("db");
    let rocksdb_path = cli.datadir.join("rocksdb");

    // Open RocksDB (read-write) with the default trie column families
    println!("Opening RocksDB at {}", rocksdb_path.display());
    let rocksdb = RocksDBBuilder::new(&rocksdb_path).with_default_tables().build()?;

    // Check if RocksDB already has trie data
    if rocksdb.first::<PackedAccountsTrie>()?.is_some() {
        println!("RocksDB already has AccountsTrie data — clearing for re-migration...");
        rocksdb.clear::<PackedAccountsTrie>()?;
        rocksdb.clear::<tables::StoragesTrie>()?;
        println!("Cleared existing trie data.");
    }

    // Open MDBX (read-only unless --clear-mdbx)
    println!("Opening MDBX at {}", db_path.display());
    if cli.clear_mdbx {
        copy_and_clear(&db_path, &rocksdb)?;
    } else {
        copy_only(&db_path, &rocksdb)?;
    }

    println!("Done! The node can now start with --storage.v2");
    Ok(())
}

/// Reads packed trie data from MDBX and writes to RocksDB.
///
/// Uses a write transaction to avoid MDBX's read-transaction timeout on large tables.
fn copy_only(db_path: &PathBuf, rocksdb: &RocksDBProvider) -> Result<()> {
    let db = open_db(db_path, DatabaseArguments::default())?;
    let tx = db.tx_mut()?;
    do_copy(&tx, rocksdb)?;
    Ok(())
}

/// Reads packed trie data from MDBX, writes to RocksDB, then clears the MDBX tables.
fn copy_and_clear(db_path: &PathBuf, rocksdb: &RocksDBProvider) -> Result<()> {
    let db = open_db(db_path, DatabaseArguments::default())?;
    let tx = db.tx_mut()?;
    do_copy(&tx, rocksdb)?;

    println!("Clearing MDBX AccountsTrie...");
    tx.clear::<tables::AccountsTrie>()?;
    println!("Clearing MDBX StoragesTrie...");
    tx.clear::<tables::StoragesTrie>()?;
    println!("Committing MDBX clear...");
    tx.commit()?;
    println!("MDBX trie tables cleared.");
    Ok(())
}

/// Core copy logic: reads from MDBX tx, writes to RocksDB.
fn do_copy<TX: DbTx>(tx: &TX, rocksdb: &RocksDBProvider) -> Result<()> {
    // -- AccountsTrie --
    println!("Reading PackedAccountsTrie from MDBX...");
    let mut batch = rocksdb.batch_with_auto_commit();
    let mut accounts_count = 0usize;
    {
        let mut cursor = tx.cursor_read::<PackedAccountsTrie>()?;
        let mut walk = cursor.walk(None)?;
        while let Some(entry) = walk.next() {
            let (key, node) = entry?;
            batch.put_account_trie(key, &node)?;
            accounts_count += 1;
            if accounts_count % 500_000 == 0 {
                println!("  AccountsTrie: {accounts_count} entries...");
            }
        }
    }
    println!("AccountsTrie: {accounts_count} entries copied");

    // -- StoragesTrie --
    println!("Reading PackedStoragesTrie from MDBX...");
    let mut storage_count = 0usize;
    {
        let mut cursor = tx.cursor_read::<PackedStoragesTrie>()?;
        let mut walk = cursor.walk(None)?;
        while let Some(entry) = walk.next() {
            let (hashed_address, packed_entry) = entry?;
            let subkey = PackedStoredNibblesSubKey(packed_entry.nibbles.0);
            batch.put_storage_trie(hashed_address, subkey, &packed_entry.node)?;
            storage_count += 1;
            if storage_count % 500_000 == 0 {
                println!("  StoragesTrie: {storage_count} entries...");
            }
        }
    }
    println!("StoragesTrie: {storage_count} entries copied");

    println!("Committing RocksDB batch...");
    batch.commit()?;

    println!("Migration complete:");
    println!("  AccountsTrie: {accounts_count} entries");
    println!("  StoragesTrie: {storage_count} entries");
    Ok(())
}
