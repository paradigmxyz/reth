use alloy_primitives::{keccak256, Address};
use clap::Parser;
use human_bytes::human_bytes;
use reth_codecs::Compact;
use reth_db_api::{cursor::DbDupCursorRO, database::Database, tables, transaction::DbTx};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDB;

/// The arguments for the `reth db account-storage` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The account address to check storage for
    address: Address,
}

impl Command {
    /// Execute `db account-storage` command
    pub fn execute<N: NodeTypesWithDB>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        let (slot_count, plain_size) = tool.provider_factory.db_ref().view(|tx| {
            let mut cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
            let mut count = 0usize;
            let mut total_value_bytes = 0usize;

            // Walk all storage entries for this address
            let walker = cursor.walk_dup(Some(self.address), None)?;
            for entry in walker {
                let (_, storage_entry) = entry?;
                count += 1;
                // StorageEntry encodes as: 32 bytes (key/subkey uncompressed) + compressed U256
                let mut buf = Vec::new();
                let entry_len = storage_entry.to_compact(&mut buf);
                total_value_bytes += entry_len;
            }

            // Add 20 bytes for the Address key (stored once per account in dupsort)
            let total_size = if count > 0 { 20 + total_value_bytes } else { 0 };

            Ok::<_, eyre::Report>((count, total_size))
        })??;

        // Estimate hashed storage size: 32-byte B256 key instead of 20-byte Address
        let hashed_size_estimate = if slot_count > 0 { plain_size + 12 } else { 0 };
        let total_estimate = plain_size + hashed_size_estimate;

        let hashed_address = keccak256(self.address);

        println!("Account: {}", self.address);
        println!("Hashed address: {hashed_address}");
        println!("Storage slots: {slot_count}");
        println!("Plain storage size: {} (estimated)", human_bytes(plain_size as f64));
        println!("Hashed storage size: {} (estimated)", human_bytes(hashed_size_estimate as f64));
        println!("Total estimated size: {}", human_bytes(total_estimate as f64));

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
