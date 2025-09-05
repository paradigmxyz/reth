use crate::common::AccessRights;
use alloy_primitives::{Address, B256, StorageKey, U256};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use reth_db::tables;
use reth_db_api::{cursor::DbCursorRO, cursor::DbDupCursorRO, transaction::DbTx};
use reth_db_common::DbTool;
use reth_primitives::keccak256;
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
    pub fn execute<E>(self, tool: &DbTool<E>) -> eyre::Result<()>
    where
        E: reth_node_api::FullNodeTypes,
    {
        let num_threads = self.threads.unwrap_or_else(|| {
            thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1)
        });

        // First, search for the address
        let found_address = self.search_address_parallel(tool, self.target_hashed_address, num_threads)?;
        
        if let Some(address) = found_address {
            println!("\nAccount found: {}", address);
            println!("Hashed address: {}", keccak256(address));
            
            // If we need to find a storage slot, search for it
            if let Some(target_slot) = self.target_hashed_storage_slot {
                println!("\nSearching for storage slot...");
                let found_slot = self.search_storage_slot_parallel(tool, address, target_slot, num_threads)?;
                
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
    fn search_address_parallel<E>(
        &self,
        tool: &DbTool<E>,
        target_hashed_address: B256,
        num_threads: usize,
    ) -> eyre::Result<Option<Address>>
    where
        E: reth_node_api::FullNodeTypes,
    {
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
                let mut cursor = tx
                    .cursor_read::<tables::PlainAccountState>()
                    .expect("failed to create cursor");
                
                let walker = cursor
                    .walk_range(start..end)
                    .expect("failed to create walker");
                
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
    
    /// Search for a storage slot in parallel for a given address
    fn search_storage_slot_parallel<E>(
        &self,
        tool: &DbTool<E>,
        address: Address,
        target_hashed_slot: B256,
        num_threads: usize,
    ) -> eyre::Result<Option<StorageKey>>
    where
        E: reth_node_api::FullNodeTypes,
    {
        let provider = tool.provider_factory.provider()?;
        let tx = provider.tx_ref();
        
        // Get all storage entries for this address to determine the total count
        let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
        let storage_entries: Vec<_> = storage_cursor
            .walk_dup(address, None)?
            .collect::<Result<Vec<_>, _>>()?;
        
        let total_slots = storage_entries.len();
        
        if total_slots == 0 {
            println!("No storage slots found for this address");
            return Ok(None);
        }
        
        println!(
            "Using {} threads to search through {} storage slots",
            num_threads, total_slots
        );
        
        // For storage slots, we'll divide the actual entries among threads
        let chunk_size = (total_slots + num_threads - 1) / num_threads;
        
        // Shared state
        let found_slot = Arc::new(Mutex::new(None));
        let found_flag = Arc::new(AtomicBool::new(false));
        
        // Create global progress bar
        let progress_bar = Arc::new(ProgressBar::new(total_slots as u64));
        progress_bar.set_style(
            ProgressStyle::with_template(
                "[{elapsed_precise}] [{wide_bar}] {pos:>7}/{len:7} ({per_sec}) ETA: {eta_precise}",
            )
            .unwrap(),
        );
        
        // Spawn threads
        let mut handles = vec![];
        let storage_entries = Arc::new(storage_entries);
        
        for thread_id in 0..num_threads {
            let found_slot = found_slot.clone();
            let found_flag = found_flag.clone();
            let progress_bar = progress_bar.clone();
            let storage_entries = storage_entries.clone();
            
            // Calculate range for this thread
            let start = thread_id * chunk_size;
            let end = ((thread_id + 1) * chunk_size).min(total_slots);
            
            if start >= total_slots {
                break;
            }
            
            let handle = thread::spawn(move || {
                for i in start..end {
                    // Check if another thread found the slot
                    if found_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    
                    let (storage_key, _) = &storage_entries[i];
                    
                    // Check if the hash matches the target
                    let hashed_slot = keccak256(B256::from(*storage_key));
                    if hashed_slot == target_hashed_slot {
                        found_flag.store(true, Ordering::Relaxed);
                        *found_slot.lock().unwrap() = Some(*storage_key);
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
        
        let result = *found_slot.lock().unwrap();
        if result.is_some() {
            progress_bar.finish_with_message("✓ Storage slot found!");
        } else {
            progress_bar.finish_with_message("✗ Storage slot not found for this address");
        }
        
        Ok(result)
    }
}