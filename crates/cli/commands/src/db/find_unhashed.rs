use alloy_primitives::{keccak256, Address, BlockNumber, B256, U256};
use clap::Parser;
use reth_db_api::{
    cursor::DbCursorRO, database::Database, models::BlockNumberAddress, tables, transaction::DbTx,
};
use reth_node_builder::NodeTypesWithDB;
use reth_primitives_traits::Account;
use reth_provider::ProviderFactory;
use reth_stages::StageId;
use reth_trie_common::Nibbles;
use tracing::info;

/// A match result from searching account change sets
#[derive(Debug, Clone)]
struct AccountMatch {
    block_number: BlockNumber,
    address: Address,
    pre_block_state: Option<Account>,
}

/// A match result from searching storage change sets
#[derive(Debug, Clone)]
struct StorageMatch {
    block_number: BlockNumber,
    address: Address,
    slot: B256,
    hashed_slot: Nibbles,
    pre_block_value: U256,
}

/// The arguments for the `reth db find-unhashed` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The hashed account address to search for (as hex string, will match as prefix)
    #[arg(long, value_name = "HEX")]
    pub account: Option<Nibbles>,

    /// The hashed storage slot key to search for (as hex string, will match as prefix)
    #[arg(long, value_name = "HEX")]
    pub slot: Option<Nibbles>,

    /// Minimum block number to search (stop searching below this block)
    #[arg(long, value_name = "BLOCK_NUMBER")]
    pub min_block: Option<BlockNumber>,

    /// Maximum block number to search (start searching from this block)
    #[arg(long, value_name = "BLOCK_NUMBER")]
    pub max_block: Option<BlockNumber>,

    /// Number of concurrent threads to use for searching
    #[arg(long, value_name = "COUNT", default_value_t = default_concurrency())]
    pub concurrency: usize,
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() * 2)
}

impl Command {
    /// Search for an account by its hash nibbles (prefix match) and output results
    fn search_account_by_nibbles<DB: Database>(
        db: &DB,
        account_prefix: Nibbles,
        min_block: Option<BlockNumber>,
        max_block: Option<BlockNumber>,
        db_tip: BlockNumber,
        concurrency: usize,
    ) -> eyre::Result<()> {
        // Determine actual min and max blocks for the range
        let actual_max = max_block.unwrap_or(db_tip);
        let actual_min = min_block.unwrap_or(0);

        if actual_min > actual_max {
            info!(
                "Min block {} is greater than max block {}, nothing to search",
                actual_min, actual_max
            );
            return Ok(());
        }

        info!(
            "Searching for account with nibbles prefix: {:?} from block {} to {} using {} threads",
            account_prefix, actual_min, actual_max, concurrency
        );

        // Calculate the block range size and split it among threads
        let total_blocks = actual_max - actual_min + 1;
        let blocks_per_thread = total_blocks.div_ceil(concurrency as u64);

        // Spawn threads to search in parallel
        let mut all_matches = std::thread::scope(|scope| {
            let mut handles = Vec::new();

            for thread_id in 0..concurrency {
                let thread_min = actual_min + (thread_id as u64 * blocks_per_thread);
                let thread_max = (thread_min + blocks_per_thread - 1).min(actual_max);

                // Skip if this thread's range is beyond the actual range
                if thread_min > actual_max {
                    break;
                }

                let handle = scope.spawn(move || -> eyre::Result<Vec<AccountMatch>> {
                    // Each thread creates its own database transaction
                    let mut tx = db.tx()?;
                    tx.disable_long_read_transaction_safety();

                    // Create a cursor over the AccountChangeSets table
                    let mut cursor = tx.cursor_dup_read::<tables::AccountChangeSets>()?;

                    // Position the cursor at the starting point for this thread's range
                    let max_key_excl = Some(thread_max + 1);
                    let mut walker = cursor.walk_back(max_key_excl)?;

                    let mut matches = Vec::new();

                    // Iterate through entries backwards within this thread's range
                    while let Some((block_number, account_before_tx)) = walker.next().transpose()? {
                        // Check if we're above our thread's max block
                        if block_number > thread_max {
                            continue;
                        }

                        // Check if we've gone below our thread's minimum block
                        if block_number < thread_min {
                            break;
                        }

                        let address = account_before_tx.address;
                        let hashed_address = keccak256(address);
                        let hashed_address_nibbles = Nibbles::unpack(hashed_address);

                        // Check if this is the account we're looking for (prefix match)
                        if hashed_address_nibbles.starts_with(&account_prefix) {
                            matches.push(AccountMatch {
                                block_number,
                                address,
                                pre_block_state: account_before_tx.info,
                            });
                        }
                    }

                    Ok(matches)
                });

                handles.push(handle);
            }

            // Wait for all threads to complete and collect results
            let mut all_matches = Vec::new();
            for handle in handles {
                let thread_matches = handle.join().unwrap()?;
                all_matches.extend(thread_matches);
            }

            Ok::<Vec<AccountMatch>, eyre::Error>(all_matches)
        })?;

        // Sort results by block number in reverse order (highest first)
        all_matches.sort_by(|a, b| b.block_number.cmp(&a.block_number));

        // Log all sorted results
        if all_matches.is_empty() {
            info!("No account found with the given hash");
        } else {
            info!("Total matches found: {}", all_matches.len());
            for match_result in all_matches {
                info!(
                    ?match_result.address,
                    ?match_result.block_number,
                    pre_block_state=?match_result.pre_block_state,
                    "Found matching account!"
                );
            }
        }
        Ok(())
    }

    /// Search for a storage slot by slot nibbles (prefix match) and optionally account hash
    fn search_storage_by_nibbles<DB: Database>(
        db: &DB,
        account_prefix: Option<Nibbles>,
        slot_prefix: Nibbles,
        min_block: Option<BlockNumber>,
        max_block: Option<BlockNumber>,
        db_tip: BlockNumber,
        concurrency: usize,
    ) -> eyre::Result<()> {
        // Determine actual min and max blocks for the range
        let actual_max = max_block.unwrap_or(db_tip);
        let actual_min = min_block.unwrap_or(0);

        if actual_min > actual_max {
            info!(
                "Min block {} is greater than max block {}, nothing to search",
                actual_min, actual_max
            );
            return Ok(());
        }

        if let Some(ref account_prefix) = account_prefix {
            info!(
                "Searching for storage slot with account nibbles prefix: {account_prefix:?} and hashed slot nibbles prefix: {slot_prefix:?} from block {} to {} using {} threads",
                actual_min, actual_max, concurrency
            );
        } else {
            info!(
                "Searching for storage slot with nibbles prefix: {slot_prefix:?} from block {} to {} using {} threads",
                actual_min, actual_max, concurrency
            );
        }

        // Calculate the block range size and split it among threads
        let total_blocks = actual_max - actual_min + 1;
        let blocks_per_thread = total_blocks.div_ceil(concurrency as u64);

        // Spawn threads to search in parallel
        let mut all_matches = std::thread::scope(|scope| {
            let mut handles = Vec::new();

            for thread_id in 0..concurrency {
                let thread_min = actual_min + (thread_id as u64 * blocks_per_thread);
                let thread_max = (thread_min + blocks_per_thread - 1).min(actual_max);

                // Skip if this thread's range is beyond the actual range
                if thread_min > actual_max {
                    break;
                }

                let handle = scope.spawn(move || -> eyre::Result<Vec<StorageMatch>> {
                    // Each thread creates its own database transaction
                    let mut tx = db.tx()?;
                    tx.disable_long_read_transaction_safety();

                    // Create a cursor over the StorageChangeSets table
                    let mut cursor = tx.cursor_dup_read::<tables::StorageChangeSets>()?;

                    // Position the cursor at the starting point for this thread's range
                    let max_key_excl = Some(BlockNumberAddress((thread_max + 1, Address::ZERO)));
                    let mut walker = cursor.walk_back(max_key_excl)?;

                    let mut matches = Vec::new();

                    // Iterate through entries backwards within this thread's range
                    while let Some((BlockNumberAddress((block_number, address)), storage_entry)) =
                        walker.next().transpose()?
                    {
                        // Check if we're above our thread's max block
                        if block_number > thread_max {
                            continue;
                        }

                        // Check if we've gone below our thread's minimum block
                        if block_number < thread_min {
                            break;
                        }

                        let hashed_slot = keccak256(storage_entry.key);
                        let hashed_slot_nibbles = Nibbles::unpack(hashed_slot);

                        // Check if the slot nibbles match as a prefix
                        let slot_matches = hashed_slot_nibbles.starts_with(&slot_prefix);
                        let addr_matches = account_prefix.as_ref().is_none_or(|prefix| {
                            let hashed_address = Nibbles::unpack(keccak256(address));
                            hashed_address.starts_with(prefix)
                        });

                        if slot_matches && addr_matches {
                            matches.push(StorageMatch {
                                block_number,
                                address,
                                slot: storage_entry.key,
                                hashed_slot: hashed_slot_nibbles,
                                pre_block_value: storage_entry.value,
                            });
                        }
                    }

                    Ok(matches)
                });

                handles.push(handle);
            }

            // Wait for all threads to complete and collect results
            let mut all_matches = Vec::new();
            for handle in handles {
                let thread_matches = handle.join().unwrap()?;
                all_matches.extend(thread_matches);
            }

            Ok::<Vec<StorageMatch>, eyre::Error>(all_matches)
        })?;

        // Sort results by block number in reverse order (highest first)
        all_matches.sort_by(|a, b| b.block_number.cmp(&a.block_number));

        // Log all sorted results
        if all_matches.is_empty() {
            info!("No storage slot found with the given criteria");
        } else {
            info!("Total matches found: {}", all_matches.len());
            for match_result in all_matches {
                info!(
                    ?match_result.address,
                    slot=?match_result.slot,
                    ?match_result.hashed_slot,
                    ?match_result.block_number,
                    pre_block_value=?match_result.pre_block_value,
                    "Found matching slot"
                );
            }
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

        // Get the db tip block from the Finish checkpoint
        let db_tip = tx
            .get::<tables::StageCheckpoints>(StageId::Finish.to_string())?
            .unwrap_or_default()
            .block_number;
        info!("DB tip block: {}", db_tip);

        // Validate: if slot is given, account must be a full B256 (64 nibbles)
        if self.slot.is_some() &&
            let Some(ref account_prefix) = self.account &&
            account_prefix.len() != 64
        {
            panic!(
                "When --slot is provided, --account must be a full B256 hash (64 nibbles), got {} nibbles",
                account_prefix.len()
            );
        }

        // Call the appropriate search function based on which arguments are provided
        match (self.account, self.slot) {
            (account_prefix, Some(slot_prefix)) => {
                // If slot is given, always search storage (with optional account filter)
                Self::search_storage_by_nibbles(
                    db,
                    account_prefix,
                    slot_prefix,
                    self.min_block,
                    self.max_block,
                    db_tip,
                    self.concurrency,
                )
            }
            (Some(account_prefix), None) => {
                // If only account is given, search accounts with prefix matching
                Self::search_account_by_nibbles(
                    db,
                    account_prefix,
                    self.min_block,
                    self.max_block,
                    db_tip,
                    self.concurrency,
                )
            }
            (None, None) => {
                // If neither is given then error.
                panic!("At least one of --account or --slot must be provided")
            }
        }
    }
}
