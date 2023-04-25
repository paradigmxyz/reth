//! Command for clearing redundant changesets.
use crate::dirs::{DbPath, MaybePlatformPath};
use clap::Parser;
use reth_db::{cursor::DbCursorRO, tables, transaction::DbTxMut};
use reth_primitives::ChainSpec;
use reth_provider::Transaction;
use reth_staged_sync::utils::{chainspec::genesis_value_parser, init::init_db};
use std::sync::Arc;

/// `reth prune-changesets` command
/// Ref https://github.com/paradigmxyz/reth/pull/2355
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(global = true, long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: MaybePlatformPath<DbPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    /// Flag wether to commit changes or not
    commit: bool,
}

impl Command {
    /// Execute `prune-changesets` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to db directory
        let db_path = self.db.unwrap_or_chain_default(self.chain.chain);

        std::fs::create_dir_all(&db_path)?;

        let db = Arc::new(init_db(db_path)?);
        let tx = Transaction::new(db.as_ref())?;

        let mut storage_changesets_cursor = tx.cursor_dup_write::<tables::StorageChangeSet>()?;

        let mut deleted_count = 0;

        let mut current_entry = storage_changesets_cursor.first()?;
        while let Some((key, entry)) = current_entry {
            // Seek all next changesets with the same value
            let mut first = true;
            let mut keys_to_delete = Vec::new();
            for next_entry in storage_changesets_cursor.walk_range(key..)? {
                if first {
                    first = false;
                    continue
                }
                let (next_key, next_entry) = next_entry?;
                if key.0 .1 == next_key.0 .1 && entry.key == next_entry.key {
                    // Found the next changeset for the same address/slot
                    if entry.value == next_entry.value {
                        // If the value is the same, we can delete the next changeset
                        keys_to_delete.push(next_key);
                    } else {
                        // If the value is different, we can stop looking for more changesets
                        break
                    }
                }
            }

            println!(
                "Found {} changesets to delete for {:?} at slot {:?}",
                keys_to_delete.len(),
                key.0 .1,
                entry.key
            );
            deleted_count += keys_to_delete.len();
            current_entry = None;
        }

        println!("Found {deleted_count} changesets to delete");

        // TODO: recalculate state root to ensure changes are correct

        if self.commit {
            // commit changes
        }

        Ok(())
    }
}
