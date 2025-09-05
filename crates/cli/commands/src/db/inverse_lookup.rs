use alloy_primitives::{keccak256, Address, StorageKey, B256, U256};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use reth_db::{tables, DatabaseEnv};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_db_common::DbTool;
use reth_primitives_traits::StorageEntry;
use reth_provider::providers::ProviderNodeTypes;
use std::{
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

/// `reth db inverse-lookup` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The target hashed address to search for (keccak256 hash of the address)
    #[arg(long, value_parser = B256::from_str, required = true)]
    target_hashed_address: B256,

    /// The target hashed storage slot to search for (keccak256 hash of the storage slot)
    /// When specified, will also search for the storage slot after finding the address
    #[arg(long, value_parser = B256::from_str)]
    target_hashed_storage_slot: Option<B256>,

    /// Number of threads to use for parallel search (defaults to number of CPU cores)
    #[arg(long)]
    threads: Option<usize>,
}

impl Command {
    /// Execute the `db inverse-lookup` command
    pub fn execute<N: ProviderNodeTypes<DB = Arc<DatabaseEnv>>>(
        self,
        tool: &DbTool<N>,
    ) -> eyre::Result<()> {
        let num_threads = self
            .threads
            .unwrap_or_else(|| thread::available_parallelism().map(|p| p.get()).unwrap_or(1));

        // First, search for the address
        let found_address =
            self.search_address_parallel(tool, self.target_hashed_address, num_threads)?;

        if let Some(address) = found_address {
            println!("\nAccount found: {}", address);
            println!("Hashed address: {}", keccak256(address));

            // If we need to find a storage slot, search for it
            if let Some(target_slot) = self.target_hashed_storage_slot {
                println!("\nSearching for storage slot...");
                let found_slot =
                    self.search_storage_slot_parallel(tool, address, target_slot, num_threads)?;

                if let Some(storage_slot) = found_slot {
                    println!("\nStorage slot found: {}", storage_slot);
                    println!("Hashed storage slot: {}", keccak256(B256::from(storage_slot)));
                } else {
                    println!("\nStorage slot not found for this account");
                }
            }
        } else {
            println!("\nAccount not found");
        }

        Ok(())
    }

    /// Search for an address in parallel
    fn search_address_parallel<N: ProviderNodeTypes<DB = Arc<DatabaseEnv>>>(
        &self,
        tool: &DbTool<N>,
        target_hashed_address: B256,
        num_threads: usize,
    ) -> eyre::Result<Option<Address>> {
        let provider = tool.provider_factory.provider()?;
        let tx = provider.tx_ref();

        // Get total number of accounts for progress tracking
        let total_accounts = tx.entries::<tables::PlainAccountState>()?;

        println!(
            "Using {} threads to search through {} accounts for hashed address: {}",
            num_threads, total_accounts, target_hashed_address
        );

        // Calculate chunk boundaries for Address space
        let max_value = alloy_primitives::U160::MAX;
        let chunk_size = max_value / alloy_primitives::U160::from(num_threads);

        // Shared state
        let found_address = Arc::new(Mutex::new(None));
        let found_flag = Arc::new(AtomicBool::new(false));

        // Create global progress bar
        let progress_bar = Arc::new(ProgressBar::new(total_accounts as u64));
        progress_bar.set_style(
            ProgressStyle::with_template(
                "[{elapsed_precise}] [{wide_bar}] {pos:>7}/{len:7} ({per_sec}) ETA: {eta_precise}",
            )
            .unwrap(),
        );

        // We need to clone the provider factory for each thread
        let provider_factory = tool.provider_factory.clone();

        // Spawn threads
        let mut handles = vec![];

        for thread_id in 0..num_threads {
            let provider_factory = provider_factory.clone();
            let found_address = found_address.clone();
            let found_flag = found_flag.clone();
            let progress_bar = progress_bar.clone();

            // Calculate range for this thread in U160 space
            let start = alloy_primitives::U160::from(thread_id) * chunk_size;
            let end = if thread_id == num_threads - 1 {
                max_value
            } else {
                start + chunk_size - alloy_primitives::U160::from(1)
            };

            let start = Address::from(start);
            let end = Address::from(end);

            let handle = thread::spawn(move || {
                let provider = provider_factory.provider().expect("failed to create provider");
                let tx = provider.tx_ref();
                let mut cursor =
                    tx.cursor_read::<tables::PlainAccountState>().expect("failed to create cursor");

                let walker = cursor.walk_range(start..end).expect("failed to create walker");

                for result in walker {
                    // Check if another thread found the address
                    if found_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    let Ok((address, _)) = result else {
                        continue;
                    };

                    // Check if the hash matches the target
                    let hashed = keccak256(address);
                    if hashed == target_hashed_address {
                        found_flag.store(true, Ordering::Relaxed);
                        *found_address.lock().unwrap() = Some(address);
                        break;
                    }

                    progress_bar.inc(1);
                }
            });

            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            let _ = handle.join();
        }

        let result = *found_address.lock().unwrap();
        if result.is_some() {
            progress_bar.finish_with_message("✓ Account found!");
        } else {
            progress_bar.finish_with_message("✗ Account not found - search completed");
        }

        Ok(result)
    }

    /// Search for a storage slot in parallel across all addresses
    fn search_storage_slot_parallel<N: ProviderNodeTypes<DB = Arc<DatabaseEnv>>>(
        &self,
        tool: &DbTool<N>,
        address: Address,
        target_hashed_slot: B256,
        num_threads: usize,
    ) -> eyre::Result<Option<StorageKey>> {
        let provider = tool.provider_factory.provider()?;
        let tx = provider.tx_ref();

        // First, let's get an estimate of total storage entries
        let total_storage_entries = tx.entries::<tables::PlainStorageState>()?;

        println!(
            "Using {} threads to search through ~{} storage entries for address {} and hashed slot: {}",
            num_threads, total_storage_entries, address, target_hashed_slot
        );

        // Split the B256 storage key space into chunks for parallel processing
        // Storage keys are B256 values, so we split that space
        let max_value = U256::MAX;
        let chunk_size = max_value / U256::from(num_threads);

        // Shared state
        let found_slot = Arc::new(Mutex::new(None));
        let found_flag = Arc::new(AtomicBool::new(false));

        // Create global progress bar
        let progress_bar = Arc::new(ProgressBar::new(total_storage_entries as u64));
        progress_bar.set_style(
            ProgressStyle::with_template(
                "[{elapsed_precise}] [{wide_bar}] {pos:>7}/{len:7} ({per_sec}) ETA: {eta_precise}",
            )
            .unwrap(),
        );

        // We need to clone the provider factory for each thread
        let provider_factory = tool.provider_factory.clone();

        // Spawn threads
        let mut handles = vec![];

        for thread_id in 0..num_threads {
            let provider_factory = provider_factory.clone();
            let found_slot = found_slot.clone();
            let found_flag = found_flag.clone();
            let progress_bar = progress_bar.clone();

            // Calculate range for this thread in U256 space
            let start = U256::from(thread_id) * chunk_size;
            let end = if thread_id == num_threads - 1 {
                max_value
            } else {
                start + chunk_size - alloy_primitives::U256::from(1)
            };

            let start_key = StorageKey::from(start);
            let end_key = StorageKey::from(end);

            let handle = thread::spawn(move || {
                let provider = provider_factory.provider().expect("failed to create provider");
                let tx = provider.tx_ref();
                let mut storage_cursor = tx
                    .cursor_dup_read::<tables::PlainStorageState>()
                    .expect("failed to create storage cursor");

                // Walk through the PlainStorageState duptable for this address
                // PlainStorageState is keyed by (Address, StorageKey)
                // We need to check storage entries for the specific address
                if let Ok(walker) = storage_cursor.walk_dup(Some(address), Some(start_key)) {
                    for result in walker {
                        // Check if another thread found the slot
                        if found_flag.load(Ordering::Relaxed) {
                            break;
                        }

                        let Ok((_, StorageEntry { key: storage_key, value: _ })) = result else {
                            continue;
                        };

                        // Check if we've exceeded our range
                        if storage_key > end_key {
                            break;
                        }

                        // Check if the hash matches the target
                        let hashed_slot = keccak256(storage_key);
                        if hashed_slot == target_hashed_slot {
                            found_flag.store(true, Ordering::Relaxed);
                            *found_slot.lock().unwrap() = Some(storage_key);
                            break;
                        }

                        progress_bar.inc(1);
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            let _ = handle.join();
        }

        let result = *found_slot.lock().unwrap();
        if result.is_some() {
            progress_bar.finish_with_message("✓ Storage slot found!");
        } else {
            progress_bar.finish_with_message("✗ Storage slot not found for this address");
        }

        Ok(result)
    }
}
