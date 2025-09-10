use alloy_primitives::{keccak256, Address, BlockNumber, B256};
use clap::Parser;
use reth_db_api::{
    cursor::DbCursorRO, database::Database, models::BlockNumberAddress, table::Table, tables,
    transaction::DbTx,
};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::ProviderFactory;
use tracing::info;

/// The arguments for the `reth db find-unhashed` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The hashed account address to search for
    #[arg(long, value_name = "HASH")]
    pub account: Option<B256>,

    /// The hashed storage slot key to search for
    #[arg(long, value_name = "HASH")]
    pub slot: Option<B256>,

    /// Minimum block number to search (stop searching below this block)
    #[arg(long, value_name = "BLOCK_NUMBER")]
    pub min_block: Option<BlockNumber>,

    /// Maximum block number to search (start searching from this block)
    #[arg(long, value_name = "BLOCK_NUMBER")]
    pub max_block: Option<BlockNumber>,
}

impl Command {
    /// Search for an account by its hash and output results
    fn search_account_by_hash<TX: DbTx>(
        tx: &TX,
        account_hash: B256,
        min_block: Option<BlockNumber>,
        max_block: Option<BlockNumber>,
    ) -> eyre::Result<()> {
        // Create a cursor over the AccountChangeSets table
        let mut cursor = tx.cursor_dup_read::<tables::AccountChangeSets>()?;

        // Position the cursor at the starting point. We start just after the configured max block
        // so that iteration starts at the end of our target range.
        let max_key_excl = max_block.map(|max| max + 1);
        let mut current = Self::position_cursor_at_start(&mut cursor, max_key_excl)?;

        info!("Searching for account with hash: {}", account_hash);
        if let Some((start_block, _)) = &current {
            info!("Starting search from block: {}", start_block);
        }

        // Iterate through entries backwards
        while let Some((block_number, account_before_tx)) = current {
            // Check if we've gone below the minimum block
            if let Some(min_block) = min_block {
                if block_number < min_block {
                    info!("Reached minimum block {}, stopping search", min_block);
                    break;
                }
            }

            let address = account_before_tx.address;
            let hashed_address = keccak256(address);

            // Check if this is the account we're looking for
            if hashed_address == account_hash {
                info!("Found matching account!");
                info!("  Account address: {}", address);
                info!("  Block number: {}", block_number);

                // Output the account's state prior to the block
                info!("  Account state before block: {:?}", account_before_tx.info);

                return Ok(());
            }

            // Move to the previous entry
            current = cursor.prev()?;
        }

        info!("No account found with the given hash");
        Ok(())
    }

    /// Search for a storage slot by slot hash and optionally account hash and output results
    fn search_storage_by_hash<TX: DbTx>(
        tx: &TX,
        account_hash: Option<B256>,
        slot_hash: B256,
        min_block: Option<BlockNumber>,
        max_block: Option<BlockNumber>,
    ) -> eyre::Result<()> {
        // Create a cursor over the StorageChangeSets table
        let mut cursor = tx.cursor_dup_read::<tables::StorageChangeSets>()?;

        // Calculate the upper limit of the address space, so that we start at the last possible key
        // for the block.
        let max_key_excl = max_block.map(|block| BlockNumberAddress((block + 1, Address::ZERO)));

        // Position the cursor at the starting point
        // For StorageChangeSets, the key is BlockNumberAddress which is (BlockNumber, Address)
        let mut current = Self::position_cursor_at_start(&mut cursor, max_key_excl)?;

        if let Some(account_hash) = account_hash {
            info!(
                "Searching for storage slot with account hash: {} and slot hash: {}",
                account_hash, slot_hash
            );
        } else {
            info!("Searching for storage slot with slot hash: {}", slot_hash);
        }

        if let Some((BlockNumberAddress((start_block, _)), _)) = &current {
            info!("Starting search from block: {}", start_block);
        }

        // Iterate through entries backwards
        while let Some((BlockNumberAddress((block_number, address)), storage_entry)) = current {
            // Check if we've gone below the minimum block
            if let Some(min_block) = min_block {
                if block_number < min_block {
                    info!("Reached minimum block {}, stopping search", min_block);
                    break;
                }
            }

            let hashed_slot = keccak256(storage_entry.key);

            // Check if the slot matches
            if hashed_slot == slot_hash {
                // If account_hash is provided, also check that it matches
                if let Some(account_hash) = account_hash {
                    let hashed_address = keccak256(address);
                    if hashed_address != account_hash {
                        // Account doesn't match, continue searching
                        current = cursor.prev()?;
                        continue;
                    }
                }

                // Found a match!
                info!("Found matching storage slot!");
                info!("  Account address: {}", address);
                info!("  Storage slot key: {}", storage_entry.key);
                info!("  Block number: {}", block_number);
                info!("  Storage value before block: {}", storage_entry.value);

                return Ok(());
            }

            // Move to the previous entry
            current = cursor.prev()?;
        }

        info!("No storage slot found with the given hashes");
        Ok(())
    }

    /// Position the cursor at the starting point, which is the key just prior to `max_key_excl`.
    fn position_cursor_at_start<T>(
        cursor: &mut impl DbCursorRO<T>,
        max_key_excl: Option<T::Key>,
    ) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T: Table,
    {
        if let Some(max_key_excl) = max_key_excl {
            // Seek to max_key or the closest key less than or equal to it
            match cursor.seek(max_key_excl.clone())? {
                Some((key, value)) => {
                    // Check if we should go to the previous entry
                    if key >= max_key_excl {
                        Ok(cursor.prev()?)
                    } else {
                        Ok(Some((key, value)))
                    }
                }
                None => {
                    // No entry at or after max_key, go to the last entry
                    Ok(cursor.last()?)
                }
            }
        } else {
            // No max_key specified, start from the last entry
            Ok(cursor.last()?)
        }
    }

    /// Execute `db find-unhashed` command
    pub fn execute<N: NodeTypesWithDB>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        // Get a database transaction in read-only mode
        let db = provider_factory.db_ref();
        let mut tx = db.tx()?;
        tx.disable_long_read_transaction_safety();

        // Call the appropriate search function based on which arguments are provided
        match (self.account, self.slot) {
            (_, Some(slot)) => {
                // If slot is given, always search storage (with optional account filter)
                Self::search_storage_by_hash(
                    &tx,
                    self.account,
                    slot,
                    self.min_block,
                    self.max_block,
                )
            }
            (Some(account), None) => {
                // If only account is given, search accounts
                Self::search_account_by_hash(&tx, account, self.min_block, self.max_block)
            }
            (None, None) => {
                // If neither is given then error.
                panic!("At least one of --account or --slot must be provided")
            }
        }
    }
}
