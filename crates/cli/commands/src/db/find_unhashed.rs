use alloy_primitives::{hex, keccak256, Address, BlockNumber, B256};
use clap::Parser;
use reth_db_api::{
    cursor::DbCursorRO, database::Database, models::BlockNumberAddress, tables, transaction::DbTx,
};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::ProviderFactory;
use reth_trie_common::Nibbles;
use tracing::info;

/// The arguments for the `reth db find-unhashed` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The hashed account address to search for
    #[arg(long, value_name = "HASH")]
    pub account: Option<B256>,

    /// The hashed storage slot key to search for (as hex string, will match as prefix)
    #[arg(long, value_name = "HEX")]
    pub slot: Option<String>,

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
        let mut walker = cursor.walk_back(max_key_excl)?;

        info!("Searching for account with hash: {}", account_hash);
        let mut matches_found = 0;

        // Iterate through entries backwards
        while let Some((block_number, account_before_tx)) = walker.next().transpose()? {
            // Check if we're above the max block, or if we've gone below the minimum block
            if let Some(max_block) = max_block &&
                block_number > max_block
            {
                continue
            } else if let Some(min_block) = min_block &&
                block_number < min_block
            {
                info!("Reached minimum block {}, stopping search", min_block);
                break;
            }

            let address = account_before_tx.address;
            let hashed_address = keccak256(address);

            // Check if this is the account we're looking for
            if hashed_address == account_hash {
                matches_found += 1;
                info!(?address, ?block_number, pre_block_state=?account_before_tx.info, "Found matching account!");
            }
        }

        if matches_found == 0 {
            info!("No account found with the given hash");
        } else {
            info!("Total matches found: {}", matches_found);
        }
        Ok(())
    }

    /// Search for a storage slot by slot nibbles (prefix match) and optionally account hash
    fn search_storage_by_nibbles<TX: DbTx>(
        tx: &TX,
        account_hash: Option<B256>,
        slot_prefix: Nibbles,
        min_block: Option<BlockNumber>,
        max_block: Option<BlockNumber>,
    ) -> eyre::Result<()> {
        // Create a cursor over the StorageChangeSets table
        let mut cursor = tx.cursor_dup_read::<tables::StorageChangeSets>()?;

        // Calculate the upper limit of the address space, so that we start at the last possible key
        // for the block.
        let max_key_excl = max_block.map(|block| BlockNumberAddress((block + 1, Address::ZERO)));
        let mut walker = cursor.walk_back(max_key_excl)?;

        if let Some(account_hash) = account_hash {
            info!(
                "Searching for storage slot with account hash: {account_hash} and hashed slot nibbles prefix: {slot_prefix:?}",
            );
        } else {
            info!("Searching for storage slot with nibbles prefix: {slot_prefix:?}");
        }

        let mut matches_found = 0;

        // Iterate through entries backwards
        while let Some((BlockNumberAddress((block_number, address)), storage_entry)) =
            walker.next().transpose()?
        {
            // Check if we're above the max block, or if we've gone below the minimum block
            if let Some(max_block) = max_block &&
                block_number > max_block
            {
                continue
            } else if let Some(min_block) = min_block &&
                block_number < min_block
            {
                info!("Reached minimum block {}, stopping search", min_block);
                break;
            }

            let hashed_slot = keccak256(storage_entry.key);
            let hashed_slot = Nibbles::unpack(hashed_slot);

            // Check if the slot nibbles match as a prefix
            let slot_matches = hashed_slot.starts_with(&slot_prefix);
            let addr_matches =
                account_hash.is_none_or(|account_hash| keccak256(address) == account_hash);

            if slot_matches && addr_matches {
                matches_found += 1;
                info!(?address, slot=?storage_entry.key, ?hashed_slot, ?block_number, pre_block_value=?storage_entry.value, "Found matching slot");
            }
        }

        if matches_found == 0 {
            info!("No storage slot found with the given criteria");
        } else {
            info!("Total matches found: {}", matches_found);
        }
        Ok(())
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
            (_, Some(slot_prefix)) => {
                let slot_prefix = Nibbles::unpack(hex::decode(slot_prefix)?);

                // If slot is given, always search storage (with optional account filter)
                Self::search_storage_by_nibbles(
                    &tx,
                    self.account,
                    slot_prefix,
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
