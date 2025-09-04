use clap::Parser;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::ProviderFactory;
use reth_trie::verify::{Inconsistency, Verifier};
use reth_trie_common::{StorageTrieEntry, StoredNibbles, StoredNibblesSubKey};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use tracing::{info, warn};

/// The arguments for the `reth db repair-trie` command
#[derive(Parser, Debug)]
pub struct Command {
    /// Only show inconsistencies without making any repairs
    #[arg(long)]
    dry_run: bool,
}

impl Command {
    /// Execute `db repair-trie` command
    pub fn execute<N: NodeTypesWithDB>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        // Get a database transaction directly from the database
        let db = provider_factory.db_ref();
        let mut tx = db.tx_mut()?;
        tx.disable_long_read_transaction_safety();

        // Create the hashed cursor factory
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&tx);

        // Create the trie cursor factory
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(&tx);

        // Create the verifier
        let verifier = Verifier::new(trie_cursor_factory, hashed_cursor_factory)?;

        let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;
        let mut storage_trie_cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;

        let mut repair_count = 0;

        // Iterate over the verifier and repair inconsistencies
        for inconsistency_result in verifier {
            let inconsistency = inconsistency_result?;
            warn!("Inconsistency found: {inconsistency:?}");
            repair_count += 1;

            if self.dry_run {
                continue;
            }

            match inconsistency {
                Inconsistency::AccountExtra(path, _node) => {
                    // Extra account node in trie, remove it
                    let nibbles = StoredNibbles(path);
                    if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
                Inconsistency::StorageExtra(account, path, _node) => {
                    // Extra storage node in trie, remove it
                    let nibbles = StoredNibblesSubKey(path);
                    if storage_trie_cursor
                        .seek_by_key_subkey(account, nibbles.clone())?
                        .filter(|e| e.nibbles == nibbles)
                        .is_some()
                    {
                        storage_trie_cursor.delete_current()?;
                    }
                }
                Inconsistency::AccountWrong { path, expected, .. } => {
                    // Wrong account node value, update it
                    let nibbles = StoredNibbles(path);
                    account_trie_cursor.upsert(nibbles, &expected)?;
                }
                Inconsistency::StorageWrong { account, path, expected, .. } => {
                    // Wrong storage node value, update it
                    let nibbles = StoredNibblesSubKey(path);
                    let entry = StorageTrieEntry { nibbles, node: expected };
                    storage_trie_cursor.upsert(account, &entry)?;
                }
                Inconsistency::AccountMissing(path, node) => {
                    // Missing account node, add it
                    let nibbles = StoredNibbles(path);
                    account_trie_cursor.upsert(nibbles, &node)?;
                }
                Inconsistency::StorageMissing(account, path, node) => {
                    // Missing storage node, add it
                    let nibbles = StoredNibblesSubKey(path);
                    let entry = StorageTrieEntry { nibbles, node };
                    storage_trie_cursor.upsert(account, &entry)?;
                }
            }
        }

        if repair_count > 0 {
            if self.dry_run {
                info!("Found {} inconsistencies (dry run - no changes made)", repair_count);
            } else {
                info!("Repaired {} inconsistencies", repair_count);
                tx.commit()?;
                info!("Changes committed to database");
            }
        } else {
            info!("No inconsistencies found");
        }

        Ok(())
    }
}
