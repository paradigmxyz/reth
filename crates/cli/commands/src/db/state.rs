use alloy_primitives::{Address, BlockNumber, B256, U256};
use clap::Parser;
use reth_db_api::{cursor::DbDupCursorRO, database::Database, tables, transaction::DbTx};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDB;
use reth_provider::providers::ProviderNodeTypes;
use reth_storage_api::{BlockNumReader, StateProvider, StorageReader, StorageSettingsCache};
use std::{
    collections::BTreeSet,
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
        let entries = tool.provider_factory.db_ref().view(|tx| {
            // Get account info
            let account = tx.get::<tables::PlainAccountState>(address)?;

            // Get storage entries
            let mut cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
            let mut entries = Vec::new();
            let mut last_log = Instant::now();

            let walker = cursor.walk_dup(Some(address), None)?;
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

            Ok::<_, eyre::Report>((account, entries))
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
        let history_in_rocksdb = storage_settings.storages_history_in_rocksdb;

        // For historical queries, enumerate keys from history indices only
        // (not PlainStorageState, which reflects current state)
        if history_in_rocksdb {
            error!(
                target: "reth::cli",
                "Historical storage queries with RocksDB backend are not yet supported. \
                 Use MDBX for storage history or query current state without --block."
            );
            return Ok(());
        }

        // Get the current tip block
        let db_provider = tool.provider_factory.provider()?;
        let tip = db_provider.best_block_number()?;

        // Collect storage keys using StorageReader which handles both MDBX and static files
        let storage_keys: BTreeSet<B256> = if tip == 0 {
            BTreeSet::new()
        } else {
            info!(
                target: "reth::cli",
                address = %address,
                tip,
                "Scanning storage changesets for address"
            );

            let all_changed = db_provider.changed_storages_with_range(0..=tip)?;
            all_changed.get(&address).cloned().unwrap_or_default()
        };

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
