use alloy_primitives::Address;
use clap::Parser;
use reth_db::Database;
use reth_db_api::{
    cursor::DbCursorRO,
    models::ShardedKey,
    tables,
    transaction::DbTx,
};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::ProviderFactory;
use serde_json::json;
use tracing::info;

/// The arguments for the `reth db inspect-account-history` command
///
/// This command walks through the AccountsHistory table for a given account address,
/// displaying all shards and their contained block numbers where the account was modified.
///
/// Example:
/// ```
/// reth db inspect-account-history 0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb8
/// ```
#[derive(Parser, Debug)]
pub struct Command {
    /// The account address to inspect (can be checksummed or lowercase)
    #[arg(value_parser = parse_address)]
    address: Address,
}

impl Command {
    /// Execute `db inspect-account-history` command
    pub fn execute<N: NodeTypesWithDB>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        // Get a read-only database transaction
        let db = provider_factory.db_ref();
        let tx = db.tx()?;

        info!("Inspecting AccountsHistory for address: {}", self.address);
        println!("AccountsHistory for address: {}", self.address);
        println!();

        // Create a cursor for the AccountsHistory table
        let mut cursor = tx.cursor_read::<tables::AccountsHistory>()?;

        // Start walking from the first possible shard for this address
        let start_key = ShardedKey::new(self.address, 0);
        let mut walker = cursor.walk(Some(start_key))?;

        let mut found_any = false;

        // Walk through all shards for this address
        while let Some(result) = walker.next() {
            let (key, block_list) = result?;

            // Stop if we've moved to a different address
            if key.key != self.address {
                break;
            }

            found_any = true;

            // Display key in JSON format
            let key_json = json!({
                "key": key.key.to_string(),
                "highest_block_number": key.highest_block_number,
            });
            println!("Key: {}", serde_json::to_string(&key_json)?);

            // Display value (block numbers)
            let blocks: Vec<u64> = block_list.iter().collect();
            println!("Value: {:?}", blocks);
            println!();
        }

        if !found_any {
            println!("No entries found for this address");
        }

        Ok(())
    }
}

/// Parse an address from a string
fn parse_address(s: &str) -> Result<Address, String> {
    s.parse::<Address>().map_err(|e| format!("Invalid address: {}", e))
}
