use alloy_primitives::{keccak256, Address, BlockNumber, B256, U256};
use clap::Parser;
use crossbeam_channel::Sender;
use reth_db_api::{
    cursor::DbCursorRO, database::Database, models::BlockNumberAddress, tables, transaction::DbTx,
};
use reth_node_builder::NodeTypesWithDB;
use reth_primitives_traits::Account;
use reth_provider::ProviderFactory;
use reth_stages::StageId;
use reth_trie_common::Nibbles;
use std::{ops::RangeInclusive, time::Duration};
use tracing::{debug, info, trace};

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

/// Distributes work chunks to worker threads via a channel and reports progress
/// Returns when all chunks have been distributed
fn distribute_work_and_report_progress(
    work_tx: Sender<RangeInclusive<BlockNumber>>,
    min_block: BlockNumber,
    max_block: BlockNumber,
    chunk_size: u64,
) {
    let total_blocks = max_block - min_block + 1;
    let total_chunks = total_blocks.div_ceil(chunk_size);
    let mut chunks_written = 0u64;
    let mut last_report = std::time::Instant::now();
    let report_interval = Duration::from_secs(2);

    let mut current_min = min_block;
    while current_min <= max_block {
        let current_max = (current_min + chunk_size - 1).min(max_block);
        let range = current_min..=current_max;

        // Send the work chunk - if this fails, all workers have panicked
        if work_tx.send(range).is_err() {
            debug!("Work channel closed, stopping distribution");
            break;
        }

        chunks_written += 1;
        current_min = current_max + 1;

        // Report progress periodically
        if last_report.elapsed() >= report_interval {
            let progress = chunks_written as f64 / total_chunks as f64;
            info!(
                "Search progress: {:.1}% ({}/{} chunks)",
                progress * 100.0,
                chunks_written,
                total_chunks
            );
            last_report = std::time::Instant::now();
        }
    }

    // Report final progress
    let progress = chunks_written as f64 / total_chunks as f64;
    info!("Search progress: {:.1}% ({}/{} chunks)", progress * 100.0, chunks_written, total_chunks);

    // Close the channel by dropping the sender
    drop(work_tx);
}

/// The arguments for the `reth db search-changesets` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The unhashed account address to search for (will be hashed automatically)
    #[arg(long, value_name = "ADDRESS", conflicts_with = "hashed_account")]
    pub account: Option<Address>,

    /// The hashed account address to search for (as hex string, will match as prefix)
    #[arg(long, value_name = "HEX", conflicts_with = "account")]
    pub hashed_account: Option<Nibbles>,

    /// The unhashed storage slot key to search for (will be hashed automatically)
    #[arg(long, value_name = "SLOT", conflicts_with = "hashed_slot")]
    pub slot: Option<B256>,

    /// The hashed storage slot key to search for (as hex string, will match as prefix)
    #[arg(long, value_name = "HEX", conflicts_with = "slot")]
    pub hashed_slot: Option<Nibbles>,

    /// Minimum block number to search (stop searching below this block)
    #[arg(long, value_name = "BLOCK_NUMBER")]
    pub min_block: Option<BlockNumber>,

    /// Maximum block number to search (start searching from this block)
    #[arg(long, value_name = "BLOCK_NUMBER")]
    pub max_block: Option<BlockNumber>,

    /// Number of concurrent threads to use for searching
    #[arg(long, value_name = "COUNT", default_value_t = default_concurrency())]
    pub concurrency: usize,

    /// Size of block range chunks to distribute to workers
    #[arg(long, value_name = "BLOCKS", default_value_t = 100)]
    pub chunk_size: u64,

    /// Output matches immediately without collecting and sorting
    #[arg(long)]
    pub unsorted: bool,
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() * 2)
}

impl Command {
    /// Search for an account by its hash nibbles (prefix match) and output results
    fn search_account_by_nibbles<DB: Database>(
        &self,
        db: &DB,
        account_prefix: Nibbles,
        db_tip: BlockNumber,
    ) -> eyre::Result<()> {
        // Determine actual min and max blocks for the range
        let actual_max = self.max_block.unwrap_or(db_tip);
        let actual_min = self.min_block.unwrap_or(0);

        if actual_min > actual_max {
            info!(
                "Min block {} is greater than max block {}, nothing to search",
                actual_min, actual_max
            );
            return Ok(());
        }

        info!(
            "Searching for account with nibbles prefix: {:?} from block {} to {} using {} threads with chunk size {}",
            account_prefix, actual_min, actual_max, self.concurrency, self.chunk_size
        );

        // Create work channel for distributing block ranges
        let (work_tx, work_rx) = crossbeam_channel::bounded::<RangeInclusive<BlockNumber>>(1);

        // Spawn threads to search in parallel
        let mut all_matches = std::thread::scope(|scope| {
            let mut handles = Vec::new();

            // Spawn worker threads
            for thread_id in 0..self.concurrency {
                let work_rx = work_rx.clone();
                let unsorted = self.unsorted;
                let handle = scope.spawn(move || -> eyre::Result<Vec<AccountMatch>> {
                    // Each thread creates its own database transaction
                    let mut tx = db.tx()?;
                    tx.disable_long_read_transaction_safety();

                    // Create a cursor over the AccountChangeSets table
                    let mut cursor = tx.cursor_dup_read::<tables::AccountChangeSets>()?;

                    let mut matches = Vec::new();

                    // Process work chunks from the channel
                    while let Ok(range) = work_rx.recv() {
                        trace!(?thread_id, range_start = ?range.start(), range_end = ?range.end(), "Processing range");

                        // Position the cursor at the starting point for this range
                        let min_key_incl = Some(*range.start());
                        let mut walker = cursor.walk(min_key_incl)?;

                        // Iterate through entries forwards within this range
                        while let Some((block_number, account_before_tx)) =
                            walker.next().transpose()?
                        {
                            // Check if we're below our range's min block
                            if block_number < *range.start() {
                                continue;
                            }

                            // Check if we've gone above our range's maximum block
                            if block_number > *range.end() {
                                break;
                            }

                            let address = account_before_tx.address;
                            let hashed_address = keccak256(address);
                            let hashed_address_nibbles = Nibbles::unpack(hashed_address);

                            // Check if this is the account we're looking for (prefix match)
                            if hashed_address_nibbles.starts_with(&account_prefix) {
                                if unsorted {
                                    // Output immediately without collecting
                                    info!(
                                        ?address,
                                        ?block_number,
                                        pre_block_state=?account_before_tx.info,
                                        "Found matching account"
                                    );
                                } else {
                                    // Collect for later sorting
                                    matches.push(AccountMatch {
                                        block_number,
                                        address,
                                        pre_block_state: account_before_tx.info,
                                    });
                                }
                            }
                        }
                    }

                    Ok(matches)
                });

                handles.push(handle);
            }

            // Distribute work and report progress inline in the main thread
            distribute_work_and_report_progress(work_tx, actual_min, actual_max, self.chunk_size);

            // Wait for all threads to complete and collect results
            let mut all_matches = Vec::new();
            for handle in handles {
                let thread_matches = handle.join().unwrap()?;
                all_matches.extend(thread_matches);
            }

            Ok::<Vec<AccountMatch>, eyre::Error>(all_matches)
        })?;

        // Only sort and output if not in unsorted mode (matches were already logged immediately)
        if !self.unsorted {
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
                        "Found matching account"
                    );
                }
            }
        }
        Ok(())
    }

    /// Search for a storage slot by slot nibbles (prefix match) and optionally account hash
    fn search_storage_by_nibbles<DB: Database>(
        &self,
        db: &DB,
        account_prefix: Option<Nibbles>,
        slot_prefix: Nibbles,
        db_tip: BlockNumber,
    ) -> eyre::Result<()> {
        // Determine actual min and max blocks for the range
        let actual_max = self.max_block.unwrap_or(db_tip);
        let actual_min = self.min_block.unwrap_or(0);

        if actual_min > actual_max {
            info!(
                "Min block {} is greater than max block {}, nothing to search",
                actual_min, actual_max
            );
            return Ok(());
        }

        if let Some(ref account_prefix) = account_prefix {
            info!(
                "Searching for storage slot with account nibbles prefix: {account_prefix:?} and hashed slot nibbles prefix: {slot_prefix:?} from block {} to {} using {} threads with chunk size {}",
                actual_min, actual_max, self.concurrency, self.chunk_size
            );
        } else {
            info!(
                "Searching for storage slot with nibbles prefix: {slot_prefix:?} from block {} to {} using {} threads with chunk size {}",
                actual_min, actual_max, self.concurrency, self.chunk_size
            );
        }

        // Create work channel for distributing block ranges
        let (work_tx, work_rx) = crossbeam_channel::bounded::<RangeInclusive<BlockNumber>>(1);

        // Spawn threads to search in parallel
        let mut all_matches = std::thread::scope(|scope| {
            let mut handles = Vec::new();

            // Spawn worker threads
            for thread_id in 0..self.concurrency {
                let work_rx = work_rx.clone();
                let unsorted = self.unsorted;
                let handle = scope.spawn(move || -> eyre::Result<Vec<StorageMatch>> {
                    // Each thread creates its own database transaction
                    let mut tx = db.tx()?;
                    tx.disable_long_read_transaction_safety();

                    // Create a cursor over the StorageChangeSets table
                    let mut cursor = tx.cursor_dup_read::<tables::StorageChangeSets>()?;

                    let mut matches = Vec::new();

                    // Process work chunks from the channel
                    while let Ok(range) = work_rx.recv() {
                        trace!(?thread_id, range_start = ?range.start(), range_end = ?range.end(), "Processing range");

                        // Position the cursor at the starting point for this range
                        let min_key_incl = Some(BlockNumberAddress((*range.start(), Address::ZERO)));
                        let mut walker = cursor.walk(min_key_incl)?;

                        // Iterate through entries forwards within this range
                        while let Some((
                            BlockNumberAddress((block_number, address)),
                            storage_entry,
                        )) = walker.next().transpose()?
                        {
                            // Check if we're below our range's min block
                            if block_number < *range.start() {
                                continue;
                            }

                            // Check if we've gone above our range's maximum block
                            if block_number > *range.end() {
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
                                if unsorted {
                                    // Output immediately without collecting
                                    info!(
                                        ?address,
                                        slot=?storage_entry.key,
                                        hashed_slot=?hashed_slot_nibbles,
                                        ?block_number,
                                        pre_block_value=?storage_entry.value,
                                        "Found matching slot"
                                    );
                                } else {
                                    // Collect for later sorting
                                    matches.push(StorageMatch {
                                        block_number,
                                        address,
                                        slot: storage_entry.key,
                                        hashed_slot: hashed_slot_nibbles,
                                        pre_block_value: storage_entry.value,
                                    });
                                }
                            }
                        }
                    }

                    Ok(matches)
                });

                handles.push(handle);
            }

            // Distribute work and report progress inline in the main thread
            distribute_work_and_report_progress(work_tx, actual_min, actual_max, self.chunk_size);

            // Wait for all threads to complete and collect results
            let mut all_matches = Vec::new();
            for handle in handles {
                let thread_matches = handle.join().unwrap()?;
                all_matches.extend(thread_matches);
            }

            Ok::<Vec<StorageMatch>, eyre::Error>(all_matches)
        })?;

        // Only sort and output if not in unsorted mode (matches were already logged immediately)
        if !self.unsorted {
            // Sort results by block number in reverse order (highest first)
            all_matches.sort_by(|a, b| b.block_number.cmp(&a.block_number));

            // Log all sorted results
            if all_matches.is_empty() {
                info!("No storage slot found with the given criteria");
            } else {
                info!("Total matches found: {}", all_matches.len());
                for match_result in all_matches {
                    info!(
                        address = ?match_result.address,
                        slot=?match_result.slot,
                        hashed_slot=?match_result.hashed_slot,
                        ?match_result.block_number,
                        pre_block_value=?match_result.pre_block_value,
                        "Found matching slot"
                    );
                }
            }
        }
        Ok(())
    }

    /// Execute `db search-changesets` command
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

        // Hash unhashed parameters if provided
        let account_prefix = if let Some(account) = self.account {
            Some(Nibbles::unpack(keccak256(account)))
        } else {
            self.hashed_account
        };

        let slot_prefix = if let Some(slot) = self.slot {
            Some(Nibbles::unpack(keccak256(slot)))
        } else {
            self.hashed_slot
        };

        // Validate: if slot is given, account must be a full B256 (64 nibbles)
        // This only applies when using --hashed-account, not --account (which is always full hash)
        if self.hashed_slot.is_some() &&
            let Some(ref hashed_account) = self.hashed_account &&
            hashed_account.len() != 64
        {
            panic!(
                "When --hashed-slot is provided, --hashed-account must be a full B256 hash (64 nibbles), got {} nibbles",
                hashed_account.len()
            );
        }

        // Call the appropriate search function based on which arguments are provided
        match (account_prefix, slot_prefix) {
            (account_prefix, Some(slot_prefix)) => {
                // If slot is given, always search storage (with optional account filter)
                self.search_storage_by_nibbles(db, account_prefix, slot_prefix, db_tip)
            }
            (Some(account_prefix), None) => {
                // If only account is given, search accounts with prefix matching
                self.search_account_by_nibbles(db, account_prefix, db_tip)
            }
            (None, None) => {
                // If neither is given then error.
                panic!("At least one of --account, --hashed-account, --slot, or --hashed-slot must be provided")
            }
        }
    }
}
