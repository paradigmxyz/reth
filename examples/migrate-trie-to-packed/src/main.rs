#![allow(unused_crate_dependencies)]

//! Migrates reth trie tables from legacy (v1, 65-byte nibble keys) to packed (v2, 33-byte nibble
//! keys).
//!
//! This reads all entries from `AccountsTrie` and `StoragesTrie` using the legacy encoding,
//! clears both tables, and rewrites them using packed encoding. Finally, it updates the
//! `StorageSettings` metadata entry to v2.
//!
//! **Back up your datadir before running this.**
//!
//! Usage:
//! ```sh
//! cargo run -p example-migrate-trie-to-packed -- --datadir ~/.local/share/reth/mainnet
//! ```

use clap::Parser;
use eyre::Result;
use reth_db::{mdbx::DatabaseArguments, open_db};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::StorageSettings,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_trie_common::{PackedStorageTrieEntry, PackedStoredNibbles, PackedStoredNibblesSubKey};
use reth_trie_db::{PackedAccountsTrie, PackedStoragesTrie};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Migrate trie tables from legacy to packed nibble encoding")]
struct Cli {
    /// Path to the reth datadir (e.g. ~/.local/share/reth/mainnet)
    #[arg(long)]
    datadir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.datadir.join("db");

    println!("Opening database at {}", db_path.display());
    let db = open_db(db_path, DatabaseArguments::default())?;
    let tx = db.tx_mut()?;

    // -- AccountsTrie --
    println!("Reading AccountsTrie (legacy encoding)...");
    let account_entries = {
        let mut cursor = tx.cursor_read::<tables::AccountsTrie>()?;
        let mut entries = Vec::new();
        let walker = cursor.walk(None)?;
        for result in walker {
            let (key, value) = result?;
            entries.push((key, value));
            if entries.len() % 100_000 == 0 {
                println!("  read {} AccountsTrie entries", entries.len());
            }
        }
        entries
    };
    let account_count = account_entries.len();
    println!("Read {account_count} AccountsTrie entries");

    println!("Clearing AccountsTrie...");
    tx.clear::<tables::AccountsTrie>()?;

    println!("Writing AccountsTrie (packed encoding)...");
    {
        let mut cursor = tx.cursor_write::<PackedAccountsTrie>()?;
        for (i, (key, value)) in account_entries.into_iter().enumerate() {
            let packed_key = PackedStoredNibbles(key.0);
            cursor.upsert(packed_key, &value)?;
            if (i + 1) % 100_000 == 0 {
                println!("  wrote {} AccountsTrie entries", i + 1);
            }
        }
    }
    println!("Wrote {account_count} AccountsTrie entries (packed)");

    // -- StoragesTrie --
    println!("Reading StoragesTrie (legacy encoding)...");
    let storage_entries = {
        let mut cursor = tx.cursor_read::<tables::StoragesTrie>()?;
        let mut entries = Vec::new();
        let walker = cursor.walk(None)?;
        for result in walker {
            let (key, value) = result?;
            entries.push((key, value));
            if entries.len() % 100_000 == 0 {
                println!("  read {} StoragesTrie entries", entries.len());
            }
        }
        entries
    };
    let storage_count = storage_entries.len();
    println!("Read {storage_count} StoragesTrie entries");

    println!("Clearing StoragesTrie...");
    tx.clear::<tables::StoragesTrie>()?;

    println!("Writing StoragesTrie (packed encoding)...");
    {
        let mut cursor = tx.cursor_write::<PackedStoragesTrie>()?;
        for (i, (key, value)) in storage_entries.into_iter().enumerate() {
            let packed_entry = PackedStorageTrieEntry {
                nibbles: PackedStoredNibblesSubKey(value.nibbles.0),
                node: value.node,
            };
            cursor.upsert(key, &packed_entry)?;
            if (i + 1) % 100_000 == 0 {
                println!("  wrote {} StoragesTrie entries", i + 1);
            }
        }
    }
    println!("Wrote {storage_count} StoragesTrie entries (packed)");

    // -- Update StorageSettings metadata --
    println!("Updating StorageSettings to v2...");
    let settings = StorageSettings::v2();
    let settings_json = serde_json::to_vec(&settings)?;
    tx.put::<tables::Metadata>("storage_settings".to_string(), settings_json)?;

    println!("Committing transaction...");
    tx.commit()?;

    println!("Migration complete!");
    println!("  AccountsTrie: {account_count} entries migrated");
    println!("  StoragesTrie: {storage_count} entries migrated");
    println!("  StorageSettings: updated to v2");

    Ok(())
}
