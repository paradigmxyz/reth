use alloy_primitives::{keccak256, Address, BlockNumber, B256, U256};
use clap::Parser;
use parking_lot::Mutex;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    tables,
    transaction::DbTx,
};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDB;
use reth_provider::providers::ProviderNodeTypes;
use reth_storage_api::{BlockNumReader, StateProvider, StorageSettingsCache};
use reth_tasks::spawn_scoped_os_thread;
use std::{
    collections::BTreeSet,
    thread,
    time::{Duration, Instant},
};
use tracing::{error, info};

/// Log progress every 5 seconds
const LOG_INTERVAL: Duration = Duration::from_secs(30);

/// The arguments for the `reth db state` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The account address to get state for
    address: Address,

    /// Block number to query state at (uses current state if not provided)
    #[arg(long, short)]
    block: Option<BlockNumber>,

    /// Maximum number of storage slots to display
    #[arg(long, short, default_value = "100")]
    limit: usize,

    /// Output format (table, json, csv)
    #[arg(long, short, default_value = "table")]
    format: OutputFormat,
}

impl Command {
    /// Execute `db state` command
    pub fn execute<N: NodeTypesWithDB + ProviderNodeTypes>(
        self,
        tool: &DbTool<N>,
    ) -> eyre::Result<()> {
        let address = self.address;
        let limit = self.limit;

        if let Some(block) = self.block {
            self.execute_historical(tool, address, block, limit)
        } else {
            self.execute_current(tool, address, limit)
        }
    }

    fn execute_current<N: NodeTypesWithDB + ProviderNodeTypes>(
        &self,
        tool: &DbTool<N>,
        address: Address,
        limit: usize,
    ) -> eyre::Result<()> {
        let use_hashed_state = tool.provider_factory.cached_storage_settings().use_hashed_state();

        let entries = tool.provider_factory.db_ref().view(|tx| {
            let (account, walker_entries) = if use_hashed_state {
                let hashed_address = keccak256(address);
                let account = tx.get::<tables::HashedAccounts>(hashed_address)?;
                let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;
                let walker = cursor.walk_dup(Some(hashed_address), None)?;
                let mut entries = Vec::new();
                let mut last_log = Instant::now();
                for (idx, entry) in walker.enumerate() {
                    let (_, storage_entry) = entry?;
                    if storage_entry.value != U256::ZERO {
                        entries.push((storage_entry.key, storage_entry.value));
                    }
                    if entries.len() >= limit {
                        break;
                    }
                    if last_log.elapsed() >= LOG_INTERVAL {
                        info!(
                            target: "reth::cli",
                            address = %address,
                            slots_scanned = idx,
                            "Scanning storage slots"
                        );
                        last_log = Instant::now();
                    }
                }
                (account, entries)
            } else {
                // Get account info
                let account = tx.get::<tables::PlainAccountState>(address)?;
                // Get storage entries
                let mut cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
                let walker = cursor.walk_dup(Some(address), None)?;
                let mut entries = Vec::new();
                let mut last_log = Instant::now();
                for (idx, entry) in walker.enumerate() {
                    let (_, storage_entry) = entry?;
                    if storage_entry.value != U256::ZERO {
                        entries.push((storage_entry.key, storage_entry.value));
                    }
                    if entries.len() >= limit {
                        break;
                    }
                    if last_log.elapsed() >= LOG_INTERVAL {
                        info!(
                            target: "reth::cli",
                            address = %address,
                            slots_scanned = idx,
                            "Scanning storage slots"
                        );
                        last_log = Instant::now();
                    }
                }
                (account, entries)
            };

            Ok::<_, eyre::Report>((account, walker_entries))
        })??;

        let (account, storage_entries) = entries;

        self.print_results(address, None, account, &storage_entries);

        Ok(())
    }

    fn execute_historical<N: NodeTypesWithDB + ProviderNodeTypes>(
        &self,
        tool: &DbTool<N>,
        address: Address,
        block: BlockNumber,
        limit: usize,
    ) -> eyre::Result<()> {
        let provider = tool.provider_factory.history_by_block_number(block)?;

        // Get account info at that block
        let account = provider.basic_account(&address)?;

        // Check storage settings to determine where history is stored
        let storage_settings = tool.provider_factory.cached_storage_settings();
        let history_in_rocksdb = storage_settings.storage_v2;

        // For historical queries, enumerate keys from history indices only
        // (not PlainStorageState, which reflects current state)
        let mut storage_keys = BTreeSet::new();

        if history_in_rocksdb {
            error!(
                target: "reth::cli",
                "Historical storage queries with RocksDB backend are not yet supported. \
                 Use MDBX for storage history or query current state without --block."
            );
            return Ok(());
        }

        // Collect keys from MDBX StorageChangeSets using parallel scanning
        self.collect_mdbx_storage_keys_parallel(tool, address, &mut storage_keys)?;

        info!(
            target: "reth::cli",
            address = %address,
            block = block,
            total_keys = storage_keys.len(),
            "Found storage keys to query"
        );

        // Now query each key at the historical block using the StateProvider
        // This handles both MDBX and RocksDB backends transparently
        let mut entries = Vec::new();
        let mut last_log = Instant::now();

        for (idx, key) in storage_keys.iter().enumerate() {
            match provider.storage(address, *key) {
                Ok(Some(value)) if value != U256::ZERO => {
                    entries.push((*key, value));
                }
                _ => {}
            }

            if entries.len() >= limit {
                break;
            }

            if last_log.elapsed() >= LOG_INTERVAL {
                info!(
                    target: "reth::cli",
                    address = %address,
                    block = block,
                    keys_total = storage_keys.len(),
                    slots_scanned = idx,
                    slots_found = entries.len(),
                    "Scanning historical storage slots"
                );
                last_log = Instant::now();
            }
        }

        self.print_results(address, Some(block), account, &entries);

        Ok(())
    }

    /// Collects storage keys from MDBX StorageChangeSets using parallel block range scanning.
    fn collect_mdbx_storage_keys_parallel<N: NodeTypesWithDB + ProviderNodeTypes>(
        &self,
        tool: &DbTool<N>,
        address: Address,
        keys: &mut BTreeSet<B256>,
    ) -> eyre::Result<()> {
        const CHUNK_SIZE: u64 = 500_000; // 500k blocks per thread
        let num_threads = std::thread::available_parallelism()
            .map(|p| p.get().saturating_sub(1).max(1))
            .unwrap_or(4);

        // Get the current tip block
        let tip = tool.provider_factory.provider()?.best_block_number()?;

        if tip == 0 {
            return Ok(());
        }

        info!(
            target: "reth::cli",
            address = %address,
            tip,
            chunk_size = CHUNK_SIZE,
            num_threads,
            "Starting parallel MDBX changeset scan"
        );

        // Shared state for collecting keys
        let collected_keys: Mutex<BTreeSet<B256>> = Mutex::new(BTreeSet::new());
        let total_entries_scanned = Mutex::new(0usize);

        // Create chunk ranges
        let mut chunks: Vec<(u64, u64)> = Vec::new();
        let mut start = 0u64;
        while start <= tip {
            let end = (start + CHUNK_SIZE - 1).min(tip);
            chunks.push((start, end));
            start = end + 1;
        }

        let chunks_ref = &chunks;
        let next_chunk = Mutex::new(0usize);
        let next_chunk_ref = &next_chunk;
        let collected_keys_ref = &collected_keys;
        let total_entries_ref = &total_entries_scanned;

        thread::scope(|s| {
            let handles: Vec<_> = (0..num_threads)
                .map(|thread_id| {
                    spawn_scoped_os_thread(s, "db-state-worker", move || {
                        loop {
                            // Get next chunk to process
                            let chunk_idx = {
                                let mut idx = next_chunk_ref.lock();
                                if *idx >= chunks_ref.len() {
                                    return Ok::<_, eyre::Report>(());
                                }
                                let current = *idx;
                                *idx += 1;
                                current
                            };

                            let (chunk_start, chunk_end) = chunks_ref[chunk_idx];

                            // Open a new read transaction for this chunk
                            tool.provider_factory.db_ref().view(|tx| {
                                tx.disable_long_read_transaction_safety();

                                let mut changeset_cursor =
                                    tx.cursor_read::<tables::StorageChangeSets>()?;
                                let start_key =
                                    reth_db_api::models::BlockNumberAddress((chunk_start, address));
                                let end_key =
                                    reth_db_api::models::BlockNumberAddress((chunk_end, address));

                                let mut local_keys = BTreeSet::new();
                                let mut entries_in_chunk = 0usize;

                                if let Ok(walker) = changeset_cursor.walk_range(start_key..=end_key)
                                {
                                    for (block_addr, storage_entry) in walker.flatten() {
                                        if block_addr.address() == address {
                                            local_keys.insert(storage_entry.key);
                                        }
                                        entries_in_chunk += 1;
                                    }
                                }

                                // Merge into global state
                                collected_keys_ref.lock().extend(local_keys);
                                *total_entries_ref.lock() += entries_in_chunk;

                                info!(
                                    target: "reth::cli",
                                    thread_id,
                                    chunk_start,
                                    chunk_end,
                                    entries_in_chunk,
                                    "Thread completed chunk"
                                );

                                Ok::<_, eyre::Report>(())
                            })??;
                        }
                    })
                })
                .collect();

            for handle in handles {
                handle.join().map_err(|_| eyre::eyre!("Thread panicked"))??;
            }

            Ok::<_, eyre::Report>(())
        })?;

        let final_keys = collected_keys.into_inner();
        let total = *total_entries_scanned.lock();

        info!(
            target: "reth::cli",
            address = %address,
            total_entries = total,
            unique_keys = final_keys.len(),
            "Finished parallel MDBX changeset scan"
        );

        keys.extend(final_keys);
        Ok(())
    }

    fn print_results(
        &self,
        address: Address,
        block: Option<BlockNumber>,
        account: Option<reth_primitives_traits::Account>,
        storage: &[(alloy_primitives::B256, U256)],
    ) {
        match self.format {
            OutputFormat::Table => {
                println!("Account: {address}");
                if let Some(b) = block {
                    println!("Block: {b}");
                } else {
                    println!("Block: latest");
                }
                println!();

                if let Some(acc) = account {
                    println!("Nonce: {}", acc.nonce);
                    println!("Balance: {} wei", acc.balance);
                    if let Some(code_hash) = acc.bytecode_hash {
                        println!("Code hash: {code_hash}");
                    }
                } else {
                    println!("Account not found");
                }

                println!();
                println!("Storage ({} slots):", storage.len());
                println!("{:-<130}", "");
                println!("{:<66} | {:<64}", "Slot", "Value");
                println!("{:-<130}", "");
                for (key, value) in storage {
                    println!("{key} | {value:#066x}");
                }
            }
            OutputFormat::Json => {
                let output = serde_json::json!({
                    "address": address.to_string(),
                    "block": block,
                    "account": account.map(|a| serde_json::json!({
                        "nonce": a.nonce,
                        "balance": a.balance.to_string(),
                        "code_hash": a.bytecode_hash.map(|h| h.to_string()),
                    })),
                    "storage": storage.iter().map(|(k, v)| {
                        serde_json::json!({
                            "key": k.to_string(),
                            "value": format!("{v:#066x}"),
                        })
                    }).collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            }
            OutputFormat::Csv => {
                println!("slot,value");
                for (key, value) in storage {
                    println!("{key},{value:#066x}");
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
    Csv,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_state_args() {
        let cmd = Command::try_parse_from([
            "state",
            "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045",
            "--block",
            "1000000",
        ])
        .unwrap();
        assert_eq!(
            cmd.address,
            "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045".parse::<Address>().unwrap()
        );
        assert_eq!(cmd.block, Some(1000000));
    }

    #[test]
    fn parse_state_args_no_block() {
        let cmd = Command::try_parse_from(["state", "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045"])
            .unwrap();
        assert_eq!(cmd.block, None);
    }
}
