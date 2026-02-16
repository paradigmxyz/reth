use alloy_primitives::{keccak256, Address};
use clap::Parser;
use human_bytes::human_bytes;
use reth_codecs::Compact;
use reth_db_api::{cursor::DbDupCursorRO, database::Database, tables, transaction::DbTx};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDB;
use reth_storage_api::StorageSettingsCache;
use std::time::{Duration, Instant};
use tracing::info;

/// Log progress every 5 seconds
const LOG_INTERVAL: Duration = Duration::from_secs(5);

/// The arguments for the `reth db account-storage` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The account address to check storage for
    address: Address,
}

impl Command {
    /// Execute `db account-storage` command
    pub fn execute<N: NodeTypesWithDB>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        let address = self.address;
        let use_hashed_state = tool.provider_factory.cached_storage_settings().use_hashed_state();

        let (slot_count, storage_size) = if use_hashed_state {
            let hashed_address = keccak256(address);
            tool.provider_factory.db_ref().view(|tx| {
                let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;
                let mut count = 0usize;
                let mut total_value_bytes = 0usize;
                let mut last_log = Instant::now();

                let walker = cursor.walk_dup(Some(hashed_address), None)?;
                for entry in walker {
                    let (_, storage_entry) = entry?;
                    count += 1;
                    let mut buf = Vec::new();
                    let entry_len = storage_entry.to_compact(&mut buf);
                    total_value_bytes += entry_len;

                    if last_log.elapsed() >= LOG_INTERVAL {
                        info!(
                            target: "reth::cli",
                            address = %address,
                            slots = count,
                            key = %storage_entry.key,
                            "Processing hashed storage slots"
                        );
                        last_log = Instant::now();
                    }
                }

                let total_size = if count > 0 { 32 + total_value_bytes } else { 0 };

                Ok::<_, eyre::Report>((count, total_size))
            })??
        } else {
            tool.provider_factory.db_ref().view(|tx| {
                let mut cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
                let mut count = 0usize;
                let mut total_value_bytes = 0usize;
                let mut last_log = Instant::now();

                // Walk all storage entries for this address
                let walker = cursor.walk_dup(Some(address), None)?;
                for entry in walker {
                    let (_, storage_entry) = entry?;
                    count += 1;
                    let mut buf = Vec::new();
                    // StorageEntry encodes as: 32 bytes (key/subkey uncompressed) + compressed U256
                    let entry_len = storage_entry.to_compact(&mut buf);
                    total_value_bytes += entry_len;

                    if last_log.elapsed() >= LOG_INTERVAL {
                        info!(
                            target: "reth::cli",
                            address = %address,
                            slots = count,
                            key = %storage_entry.key,
                            "Processing storage slots"
                        );
                        last_log = Instant::now();
                    }
                }

                // Add 20 bytes for the Address key (stored once per account in dupsort)
                let total_size = if count > 0 { 20 + total_value_bytes } else { 0 };

                Ok::<_, eyre::Report>((count, total_size))
            })??
        };

        let hashed_address = keccak256(address);

        println!("Account: {address}");
        println!("Hashed address: {hashed_address}");
        println!("Storage slots: {slot_count}");
        if use_hashed_state {
            println!("Hashed storage size: {} (estimated)", human_bytes(storage_size as f64));
        } else {
            // Estimate hashed storage size: 32-byte B256 key instead of 20-byte Address
            let hashed_size_estimate = if slot_count > 0 { storage_size + 12 } else { 0 };
            let total_estimate = storage_size + hashed_size_estimate;
            println!("Plain storage size: {} (estimated)", human_bytes(storage_size as f64));
            println!(
                "Hashed storage size: {} (estimated)",
                human_bytes(hashed_size_estimate as f64)
            );
            println!("Total estimated size: {}", human_bytes(total_estimate as f64));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_address_arg() {
        let cmd = Command::try_parse_from([
            "account-storage",
            "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045",
        ])
        .unwrap();
        assert_eq!(
            cmd.address,
            "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045".parse::<Address>().unwrap()
        );
    }
}
