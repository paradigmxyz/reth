use clap::Parser;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::ProviderFactory;
use reth_trie::{
    verify::{Output, Verifier},
    Nibbles,
};
use reth_trie_common::{StorageTrieEntry, StoredNibbles, StoredNibblesSubKey};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::time::{Duration, Instant};
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

        let mut inconsistent_nodes = 0;
        let start_time = Instant::now();
        let mut last_progress_time = Instant::now();

        // Iterate over the verifier and repair inconsistencies
        for output_result in verifier {
            let output = output_result?;

            if let Output::Progress(path) = output {
                // Output progress every 5 seconds
                if last_progress_time.elapsed() > Duration::from_secs(5) {
                    output_progress(path, start_time, inconsistent_nodes);
                    last_progress_time = Instant::now();
                }
                continue
            };

            warn!("Inconsistency found, will repair: {output:?}");
            inconsistent_nodes += 1;

            if self.dry_run {
                continue;
            }

            match output {
                Output::AccountExtra(path, _node) => {
                    // Extra account node in trie, remove it
                    let nibbles = StoredNibbles(path);
                    if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
                Output::StorageExtra(account, path, _node) => {
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
                Output::AccountWrong { path, expected: node, .. } |
                Output::AccountMissing(path, node) => {
                    // Wrong/missing account node value, upsert it
                    let nibbles = StoredNibbles(path);
                    account_trie_cursor.upsert(nibbles, &node)?;
                }
                Output::StorageWrong { account, path, expected: node, .. } |
                Output::StorageMissing(account, path, node) => {
                    // Wrong/missing storage node value, upsert it
                    let nibbles = StoredNibblesSubKey(path);
                    let entry = StorageTrieEntry { nibbles, node };
                    storage_trie_cursor.upsert(account, &entry)?;
                }
                Output::Progress(_) => {
                    unreachable!()
                }
            }
        }

        if inconsistent_nodes > 0 {
            if self.dry_run {
                info!("Found {} inconsistencies (dry run - no changes made)", inconsistent_nodes);
            } else {
                info!("Repaired {} inconsistencies", inconsistent_nodes);
                tx.commit()?;
                info!("Changes committed to database");
            }
        } else {
            info!("No inconsistencies found");
        }

        Ok(())
    }
}

/// Output progress information based on the last seen account path.
fn output_progress(last_account: Nibbles, start_time: Instant, inconsistent_nodes: u64) {
    // Calculate percentage based on position in the trie path space
    // For progress estimation, we'll use the first few nibbles as an approximation

    // Convert the first 16 nibbles (8 bytes) to a u64 for progress calculation
    let mut current_value: u64 = 0;
    let nibbles_to_use = last_account.len().min(16);

    for i in 0..nibbles_to_use {
        current_value = (current_value << 4) | (last_account.get(i).unwrap_or(0) as u64);
    }
    // Shift left to fill remaining bits if we have fewer than 16 nibbles
    if nibbles_to_use < 16 {
        current_value <<= (16 - nibbles_to_use) * 4;
    }

    let progress_percent = current_value as f64 / u64::MAX as f64 * 100.0;
    let progress_percent_str = format!("{progress_percent:.2}");

    // Calculate ETA based on current speed
    let elapsed = start_time.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();

    let estimated_total_time =
        if progress_percent > 0.0 { elapsed_secs / (progress_percent / 100.0) } else { 0.0 };
    let remaining_time = estimated_total_time - elapsed_secs;
    let eta_duration = Duration::from_secs(remaining_time as u64);

    info!(
        progress_percent = progress_percent_str,
        eta = %humantime::format_duration(eta_duration),
        inconsistent_nodes,
        "Repairing trie tables",
    );
}
