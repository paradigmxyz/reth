use alloy_primitives::{keccak256, Address, StorageKey, B256, U256};
use clap::{Parser, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use reth_db::{tables, DatabaseEnv};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_db_common::DbTool;
use reth_primitives_traits::StorageEntry;
use reth_provider::providers::ProviderNodeTypes;
use reth_trie::Nibbles;
use std::{
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

/// Search target type for inverse lookup
#[derive(Debug, Clone, Copy)]
pub enum SearchTarget {
    /// Exact hash match (keccak256)
    Hash(B256),
    /// Prefix match using nibbles
    Prefix(Nibbles),
}

impl FromStr for SearchTarget {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // If it starts with "0x" and is 66 chars (0x + 64 hex chars), treat as hash
        if s.starts_with("0x") && s.len() == 66 {
            Ok(SearchTarget::Hash(B256::from_str(s)?))
        } else {
            // Otherwise, treat as a hex string prefix for nibbles
            let hex_str = s.strip_prefix("0x").unwrap_or(s);

            // Parse each hex character as a separate nibble
            let mut nibbles = Nibbles::default();

            for hex_char in hex_str.chars() {
                // Parse each hex character as a nibble (0-15)
                let nibble = u8::from_str_radix(&hex_char.to_string(), 16)?;
                if nibble > 15 {
                    return Err(eyre::eyre!("Invalid hex character: {}", hex_char));
                }
                nibbles.push(nibble);
            }

            Ok(SearchTarget::Prefix(nibbles))
        }
    }
}

/// `reth db inverse-lookup` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The target address to search for. Can be:
    /// - Full hash: 0x1234...abcd (66 chars) for exact keccak256 hash match
    /// - Prefix: 0x1234 or 1234 for nibbles prefix match
    #[arg(long, value_parser = SearchTarget::from_str, required = true)]
    target_address: SearchTarget,

    /// The target storage slot to search for. Can be:
    /// - Full hash: 0x1234...abcd (66 chars) for exact keccak256 hash match
    /// - Prefix: 0x1234 or 1234 for nibbles prefix match
    ///
    /// When specified, will also search for the storage slot after finding the address
    #[arg(long, value_parser = SearchTarget::from_str)]
    target_storage_slot: Option<SearchTarget>,

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
            self.search_address_parallel(tool, &self.target_address, num_threads)?;

        if let Some(address) = found_address {
            println!("\nAccount found: {}", address);
            println!("Hashed address: {}", keccak256(address));

            // If we need to find a storage slot, search for it
            if let Some(ref target_slot) = self.target_storage_slot {
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

    /// Check if a hash matches the search target
    fn matches_target(hash: B256, target: &SearchTarget) -> bool {
        match target {
            SearchTarget::Hash(target_hash) => hash == *target_hash,
            SearchTarget::Prefix(prefix) => {
                let hash_nibbles = Nibbles::unpack(hash.as_slice());
                hash_nibbles.starts_with(prefix)
            }
        }
    }

    /// Search for an address in parallel
    fn search_address_parallel<N: ProviderNodeTypes<DB = Arc<DatabaseEnv>>>(
        &self,
        tool: &DbTool<N>,
        target: &SearchTarget,
        num_threads: usize,
    ) -> eyre::Result<Option<Address>> {
        let provider = tool.provider_factory.provider()?;
        let tx = provider.tx_ref();

        // Get total number of accounts for progress tracking
        let total_accounts = tx.entries::<tables::PlainAccountState>()?;

        match target {
            SearchTarget::Hash(hash) => println!(
                "Using {} threads to search through {} accounts for hashed address: {}",
                num_threads, total_accounts, hash
            ),
            SearchTarget::Prefix(prefix) => println!(
                "Using {} threads to search through {} accounts for address prefix: {:?}",
                num_threads, total_accounts, prefix
            ),
        }

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
            let target = *target;

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
                    if Self::matches_target(hashed, &target) {
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
        target: &SearchTarget,
        num_threads: usize,
    ) -> eyre::Result<Option<StorageKey>> {
        let provider = tool.provider_factory.provider()?;
        let tx = provider.tx_ref();

        // First, let's get an estimate of total storage entries
        let total_storage_entries = tx.entries::<tables::PlainStorageState>()?;

        match target {
            SearchTarget::Hash(hash) => println!(
                "Using {} threads to search through ~{} storage entries for address {} and hashed slot: {}",
                num_threads, total_storage_entries, address, hash
            ),
            SearchTarget::Prefix(prefix) => println!(
                "Using {} threads to search through ~{} storage entries for address {} and slot prefix: {:?}",
                num_threads, total_storage_entries, address, prefix
            ),
        }

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
            let target = *target;

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
                        if Self::matches_target(hashed_slot, &target) {
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
