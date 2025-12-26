use alloy_primitives::{Address, B256, U256, keccak256, address};
use revm::Database;
use std::collections::HashMap;

/// ArbOS State address - the fictional account that stores ArbOS state
/// This is address 0xA4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF (as per Go nitro)
const ARBOS_STATE_ADDRESS: Address = address!("A4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF");

/// Calculates a storage slot using the same keccak256-based mapping as Solidity maps.
/// This matches the calculation in header.rs:storage_key_map
fn storage_key_map(storage_key: &[u8], offset: u64) -> U256 {
    let boundary = 31usize;

    // Convert offset to a 32-byte key (BE format with the offset in the last 8 bytes)
    // This must match uint_to_hash_u64_be in header.rs
    let mut key_bytes = [0u8; 32];
    key_bytes[24..32].copy_from_slice(&offset.to_be_bytes());

    let mut data = Vec::with_capacity(storage_key.len() + boundary);
    data.extend_from_slice(storage_key);
    data.extend_from_slice(&key_bytes[..boundary]);
    let h = keccak256(&data);
    let mut mapped = [0u8; 32];
    mapped[..boundary].copy_from_slice(&h.0[..boundary]);
    mapped[boundary] = key_bytes[boundary];
    U256::from_be_bytes(mapped)
}

/// Ensures the ArbOS account exists in bundle_state.state.
///
/// CRITICAL FOR DETERMINISM: We use state.database.basic() instead of state.basic()
/// because the latter creates cache entries that can cause non-determinism between
/// assembly and validation. By reading directly from the database, we get the correct
/// account info (including nonce=1 from genesis) without polluting the cache.
fn ensure_arbos_account_in_bundle<D: Database>(state: &mut revm::database::State<D>) {
    use revm_database::{BundleAccount, AccountStatus};
    use revm_state::AccountInfo;

    if !state.bundle_state.state.contains_key(&ARBOS_STATE_ADDRESS) {
        // Read the account info from the database directly to get the correct nonce.
        // We use state.database.basic() instead of state.basic() to avoid creating
        // cache entries that could cause non-determinism.
        // The ArbOS storage backing account has nonce=1 in genesis.
        let db_info = state.database.basic(ARBOS_STATE_ADDRESS).ok().flatten();

        tracing::info!(
            target: "arb-storage",
            "ensure_arbos_account_in_bundle: db_info={:?}",
            db_info.as_ref().map(|i| (i.nonce, i.balance, i.code_hash))
        );

        let info = db_info.map(|i| i.clone()).or_else(|| Some(AccountInfo {
            balance: U256::ZERO,
            nonce: 1, // Default to 1 if not found, matching genesis state
            code_hash: alloy_primitives::keccak256([]),
            code: None,
        }));

        // Use Loaded status initially since we're just loading from DB.
        // The status will be updated to Changed when we actually write storage.
        let acc = BundleAccount {
            info: info.clone(),
            storage: HashMap::default(),
            original_info: info, // Set original_info to track that this is from DB
            status: AccountStatus::Loaded,
        };
        state.bundle_state.state.insert(ARBOS_STATE_ADDRESS, acc);
    }
}

/// Helper function to write storage for the ArbOS account.
///
/// CRITICAL FIX: This now uses the proper transition mechanism instead of writing
/// directly to bundle_state.state. The previous approach was causing storage to be
/// lost because merge_transitions() creates bundle_state from transition_state,
/// NOT from the existing bundle_state.state.
///
/// The correct flow is:
/// 1. Load account into cache
/// 2. Modify cache entry with storage change
/// 3. Create TransitionAccount with storage changes
/// 4. Call apply_transition() to register the transition
///
/// This ensures storage changes are properly tracked and persisted.
/// Writes a storage slot to the ArbOS account using the proper transition mechanism.
/// This is the correct way to persist storage changes that will survive merge_transitions().
pub fn write_arbos_storage<D: Database>(
    state: &mut revm::database::State<D>,
    slot: U256,
    value: U256,
) {
    use revm_database::AccountStatus;

    // Step 1: Load the ArbOS account into cache
    let _ = state.load_cache_account(ARBOS_STATE_ADDRESS);

    // Step 2: Get the CURRENT value (including in-flight changes) and the ORIGINAL value from database.
    // Go nitro's SetState checks: if prev == value { return prev } where prev is the current value.
    // We need to check against the current value (cache/bundle_state), not just the database value.
    
    // First, get the current value from cache or bundle_state (in-flight changes)
    let current_value = {
        // Check cache first (most recent in-flight changes)
        if let Some(cached_acc) = state.cache.accounts.get(&ARBOS_STATE_ADDRESS) {
            if let Some(ref account) = cached_acc.account {
                if let Some(&cached_value) = account.storage.get(&slot) {
                    Some(cached_value)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }.or_else(|| {
        // Then check bundle_state (merged changes from previous transactions)
        if let Some(acc) = state.bundle_state.state.get(&ARBOS_STATE_ADDRESS) {
            if let Some(slot_entry) = acc.storage.get(&slot) {
                Some(slot_entry.present_value)
            } else {
                None
            }
        } else {
            None
        }
    });
    
    // Get original value from database for proper tracking in StorageSlot
    let original_value = state.database.storage(ARBOS_STATE_ADDRESS, slot).unwrap_or(U256::ZERO);
    
    // CRITICAL FIX: Skip no-op writes where value == current_value
    // This matches Go nitro's SetState behavior which returns early if prev == value.
    // We compare against current_value (including in-flight changes), not original_value.
    // This prevents dirtying the state trie with unchanged slots.
    let prev_value = current_value.unwrap_or(original_value);
    if value == prev_value {
        return;
    }

    // Step 3: Modify the cache entry with the storage change
    let (previous_info, previous_status, current_info, current_status) = {
        let cached_acc = match state.cache.accounts.get_mut(&ARBOS_STATE_ADDRESS) {
            Some(acc) => acc,
            None => {
                tracing::error!(
                    target: "arb-storage",
                    "[STORAGE-WRITE] ArbOS account not found in cache after load_cache_account!"
                );
                return;
            }
        };

        let previous_status = cached_acc.status;
        let previous_info = cached_acc.account.as_ref().map(|a| a.info.clone());

        // Insert the storage slot into the cache account
        // Cache storage uses plain U256 values (PlainStorage = HashMap<U256, U256>)
        if let Some(ref mut account) = cached_acc.account {
            account.storage.insert(slot, value);
        }

        // Update status to Changed
        let had_no_nonce_and_code = previous_info
            .as_ref()
            .map(|info| info.has_no_code_and_nonce())
            .unwrap_or_default();
        cached_acc.status = cached_acc.status.on_changed(had_no_nonce_and_code);

        let current_info = cached_acc.account.as_ref().map(|a| a.info.clone());
        let current_status = cached_acc.status;

        (previous_info, previous_status, current_info, current_status)
    };

    // Step 4: Create and apply the transition with storage changes
    // TransitionAccount.storage uses StorageWithOriginalValues = HashMap<U256, StorageSlot>
    use revm_database::states::StorageSlot;
    let mut storage_changes: revm_database::StorageWithOriginalValues = HashMap::default();
    storage_changes.insert(
        slot,
        StorageSlot::new_changed(original_value, value),
    );

    let transition = revm::database::TransitionAccount {
        info: current_info,
        status: current_status,
        previous_info,
        previous_status,
        storage: storage_changes,
        storage_was_destroyed: false,
    };

    state.apply_transition(vec![(ARBOS_STATE_ADDRESS, transition)]);

    tracing::warn!(
        target: "arb-storage",
        "[STORAGE-WRITE] write_arbos_storage: slot={:?} value={:?} original={:?} status={:?}",
        slot, value, original_value, current_status
    );
}

/// Computes the sendMerkle substorage key.
/// sendMerkleSubspace = []byte{5} in Go nitro
pub fn compute_send_merkle_key() -> Vec<u8> {
    keccak256(&[5u8]).to_vec()
}

/// Computes a storage slot for the sendMerkle accumulator.
/// offset 0 = size, offset 2+ = partials
pub fn compute_send_merkle_slot(offset: u64) -> U256 {
    let merkle_key = compute_send_merkle_key();
    storage_key_map(&merkle_key, offset)
}

/// Applies ArbSys Merkle accumulator state changes using the proper transition mechanism.
/// This should be called AFTER EVM execution to ensure the state changes persist.
///
/// Parameters:
/// - state: The REVM state to apply changes to
/// - new_size: The new size of the Merkle accumulator
/// - partials: Vec of (level, hash) pairs for partial hashes to set
pub fn apply_arbsys_merkle_state<D: Database>(
    state: &mut revm::database::State<D>,
    new_size: u64,
    partials: Vec<(u64, B256)>,
) {
    // Write the new size
    let size_slot = compute_send_merkle_slot(0);
    write_arbos_storage(state, size_slot, U256::from(new_size));
    
    tracing::warn!(
        target: "arb-storage",
        "[ARBSYS-MERKLE] Applied size={} to slot={:?}",
        new_size, size_slot
    );
    
    // Write the partials
    for (level, hash) in partials {
        let partial_slot = compute_send_merkle_slot(2 + level);
        let value = U256::from_be_bytes(hash.0);
        write_arbos_storage(state, partial_slot, value);
        
        tracing::warn!(
            target: "arb-storage",
            "[ARBSYS-MERKLE] Applied partial level={} hash={:?} to slot={:?}",
            level, hash, partial_slot
        );
    }
}

/// Burns (subtracts) a value from the ArbSys account balance using the proper transition mechanism.
/// This is the correct way to persist balance changes that will survive merge_transitions().
pub fn burn_arbsys_balance<D: Database>(
    state: &mut revm::database::State<D>,
    value_to_burn: U256,
) {
    use revm_database::AccountStatus;
    
    let arbsys_address = address!("0000000000000000000000000000000000000064");
    
    // Step 1: Load the ArbSys account into cache
    let _ = state.load_cache_account(arbsys_address);
    
    // Step 2: Get the current balance and modify it
    let (previous_info, previous_status, current_info, current_status) = {
        let cached_acc = match state.cache.accounts.get_mut(&arbsys_address) {
            Some(acc) => acc,
            None => {
                tracing::error!(
                    target: "arb-storage",
                    "[BURN-ARBSYS] ArbSys account not found in cache after load_cache_account!"
                );
                return;
            }
        };

        let previous_status = cached_acc.status;
        let previous_info = cached_acc.account.as_ref().map(|a| a.info.clone());
        
        // Get current balance and subtract the burn value
        let old_balance = cached_acc.account.as_ref().map(|a| a.info.balance).unwrap_or(U256::ZERO);
        let new_balance = old_balance.saturating_sub(value_to_burn);
        
        tracing::info!(
            target: "arb-storage",
            "[BURN-ARBSYS] Burning value from ArbSys: old_balance={:?} value_to_burn={:?} new_balance={:?}",
            old_balance, value_to_burn, new_balance
        );
        
        // Update the balance in the cache
        if let Some(ref mut account) = cached_acc.account {
            account.info.balance = new_balance;
        }

        // Update status to Changed
        let had_no_nonce_and_code = previous_info
            .as_ref()
            .map(|info| info.has_no_code_and_nonce())
            .unwrap_or_default();
        cached_acc.status = cached_acc.status.on_changed(had_no_nonce_and_code);

        let current_info = cached_acc.account.as_ref().map(|a| a.info.clone());
        let current_status = cached_acc.status;

        (previous_info, previous_status, current_info, current_status)
    };

    // Step 3: Create and apply the transition with balance change (no storage changes)
    let transition = revm::database::TransitionAccount {
        info: current_info.clone(),
        status: current_status,
        previous_info,
        previous_status,
        storage: std::collections::HashMap::default(),
        storage_was_destroyed: false,
    };

    state.apply_transition(vec![(arbsys_address, transition)]);

    tracing::info!(
        target: "arb-storage",
        "[BURN-ARBSYS] Applied balance burn transition: new_balance={:?} status={:?}",
        current_info.map(|i| i.balance), current_status
    );
}

/// Reads the current sendMerkle size from the state.
pub fn read_send_merkle_size<D: Database>(state: &mut revm::database::State<D>) -> u64 {
    let size_slot = compute_send_merkle_slot(0);
    
    // Check cache first
    if let Some(cached_acc) = state.cache.accounts.get(&ARBOS_STATE_ADDRESS) {
        if let Some(ref account) = cached_acc.account {
            if let Some(&cached_value) = account.storage.get(&size_slot) {
                return cached_value.try_into().unwrap_or(0);
            }
        }
    }
    
    // Then check bundle_state
    if let Some(acc) = state.bundle_state.state.get(&ARBOS_STATE_ADDRESS) {
        if let Some(slot_entry) = acc.storage.get(&size_slot) {
            return slot_entry.present_value.try_into().unwrap_or(0);
        }
    }
    
    // Finally check database
    state.database.storage(ARBOS_STATE_ADDRESS, size_slot)
        .ok()
        .map(|v| v.try_into().unwrap_or(0))
        .unwrap_or(0)
}

/// Reads a partial hash from the sendMerkle accumulator.
pub fn read_send_merkle_partial<D: Database>(state: &mut revm::database::State<D>, level: u64) -> B256 {
    let partial_slot = compute_send_merkle_slot(2 + level);
    
    // Check cache first
    if let Some(cached_acc) = state.cache.accounts.get(&ARBOS_STATE_ADDRESS) {
        if let Some(ref account) = cached_acc.account {
            if let Some(&cached_value) = account.storage.get(&partial_slot) {
                return B256::from(cached_value);
            }
        }
    }
    
    // Then check bundle_state
    if let Some(acc) = state.bundle_state.state.get(&ARBOS_STATE_ADDRESS) {
        if let Some(slot_entry) = acc.storage.get(&partial_slot) {
            return B256::from(slot_entry.present_value);
        }
    }
    
    // Finally check database
    state.database.storage(ARBOS_STATE_ADDRESS, partial_slot)
        .ok()
        .map(|v| B256::from(v))
        .unwrap_or(B256::ZERO)
}

/// Logs the full ArbOS account state for debugging.
/// Call this before state root computation to verify storage is being tracked.
pub fn log_arbos_bundle_state<D: Database>(state: &revm::database::State<D>) {
    if let Some(acc) = state.bundle_state.state.get(&ARBOS_STATE_ADDRESS) {
        tracing::warn!(
            target: "arb-storage",
            "[ARBOS-BUNDLE] ArbOS account status={:?} info_present={} storage_slots={} nonce={}",
            acc.status,
            acc.info.is_some(),
            acc.storage.len(),
            acc.info.as_ref().map(|i| i.nonce).unwrap_or(0)
        );
        for (slot, slot_entry) in acc.storage.iter() {
            tracing::warn!(
                target: "arb-storage",
                "[ARBOS-BUNDLE] slot={:?} present_value={:?} original={:?}",
                slot, slot_entry.present_value, slot_entry.previous_or_original_value
            );
        }
    } else {
        tracing::warn!(
            target: "arb-storage",
            "[ARBOS-BUNDLE] ArbOS account NOT FOUND in bundle_state.state!"
        );
    }
}


pub struct Storage<D> {
    pub state: *mut revm::database::State<D>,
    pub base_key: B256,
}

impl<D: Database> Storage<D> {
    pub fn new(state: *mut revm::database::State<D>, base_key: B256) -> Self {
        Self { state, base_key }
    }

    pub fn open_sub_storage(&self, sub_key: &[u8]) -> Storage<D> {
        // CRITICAL FIX: Go nitro uses empty slice for root storage, not 32 zero bytes!
        // When base_key is B256::ZERO (root storage), use empty slice for keccak256
        // This ensures subspace key computation matches Go nitro's storage.OpenSubStorage
        let base_slice: &[u8] = if self.base_key == B256::ZERO {
            &[]  // Empty slice for root storage
        } else {
            self.base_key.as_slice()
        };
        let mut combined = Vec::with_capacity(base_slice.len() + sub_key.len());
        combined.extend_from_slice(base_slice);
        combined.extend_from_slice(sub_key);
        let new_key = alloy_primitives::keccak256(&combined);
        Storage::new(self.state, new_key)
    }

    pub fn get_by_uint64(&self, offset: u64) -> Result<B256, ()> {
        let slot = self.compute_slot(offset);
        unsafe {
            let state = &mut *self.state;
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache for in-flight changes (writes go to cache first)
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&slot) {
                        return Ok(B256::from(value));
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&slot) {
                    return Ok(B256::from(slot_entry.present_value));
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, slot) {
                Ok(value) => Ok(B256::from(value)),
                Err(_) => Err(()),
            }
        }
    }

    pub fn set_by_uint64(&self, offset: u64, value: B256) -> Result<(), ()> {
        let slot = self.compute_slot(offset);
        let value_u256 = U256::from_be_bytes(value.0);
        unsafe {
            let state = &mut *self.state;
            write_arbos_storage(state, slot, value_u256);
            Ok(())
        }
    }

    fn compute_slot(&self, offset: u64) -> U256 {
        // Use the same storage_key_map calculation as in header.rs to ensure
        // writes and reads use the same slots
        // IMPORTANT: Go nitro uses an empty []byte{} for root storage, not 32 zero bytes!
        let storage_key = if self.base_key == B256::ZERO {
            &[] as &[u8]  // Empty slice for root storage
        } else {
            self.base_key.as_slice()
        };
        storage_key_map(storage_key, offset)
    }

    /// Computes the physical storage slot for an arbitrary B256 key using Go nitro's mapAddress algorithm.
    /// This is different from Solidity's mapping formula - it preserves the last byte and hashes only the first 31 bytes.
    fn compute_slot_for_key(&self, key: B256) -> U256 {
        let boundary = 31usize;
        let key_bytes = key.as_slice();
        
        // IMPORTANT: Go nitro uses an empty []byte{} for root storage, not 32 zero bytes!
        let storage_key: &[u8] = if self.base_key == B256::ZERO {
            &[]  // Empty slice for root storage
        } else {
            self.base_key.as_slice()
        };
        
        // Go nitro's mapAddress: keccak256(storageKey, key[:31])[:31] || key[31]
        let mut data = Vec::with_capacity(storage_key.len() + boundary);
        data.extend_from_slice(storage_key);
        data.extend_from_slice(&key_bytes[..boundary]);
        let h = keccak256(&data);
        let mut mapped = [0u8; 32];
        mapped[..boundary].copy_from_slice(&h.0[..boundary]);
        mapped[boundary] = key_bytes[boundary];
        U256::from_be_bytes(mapped)
    }

    pub fn get(&self, key: B256) -> Result<B256, ()> {
        unsafe {
            let state = &mut *self.state;
            let arbos_addr = ARBOS_STATE_ADDRESS;
            // Use mapAddress algorithm to compute the physical storage slot
            let slot = self.compute_slot_for_key(key);

            // First check cache for in-flight changes (writes go to cache first)
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&slot) {
                        return Ok(B256::from(value));
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&slot) {
                    return Ok(B256::from(slot_entry.present_value));
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, slot) {
                Ok(value) => Ok(B256::from(value)),
                Err(_) => Ok(B256::ZERO),
            }
        }
    }

    pub fn set(&self, key: B256, value: B256) -> Result<(), ()> {
        // Use mapAddress algorithm to compute the physical storage slot
        let slot = self.compute_slot_for_key(key);
        let value_u256 = U256::from_be_bytes(value.0);
        unsafe {
            let state = &mut *self.state;
            write_arbos_storage(state, slot, value_u256);
            Ok(())
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            state: self.state,
            base_key: self.base_key,
        }
    }
}

impl<D> Clone for Storage<D> {
    fn clone(&self) -> Self {
        Self {
            state: self.state,
            base_key: self.base_key,
        }
    }
}

pub struct StorageBackedUint64<D> {
    pub storage: *mut revm::database::State<D>,
    pub slot: U256,
}

impl<D: Database> StorageBackedUint64<D> {
    pub fn new(storage: *mut revm::database::State<D>, base_key: B256, offset: u64) -> Self {
        // Use the same keccak256-based slot mapping as Go nitro's Storage.NewSlot
        // IMPORTANT: Go nitro uses an empty []byte{} for root storage, not 32 zero bytes!
        let storage_key = if base_key == B256::ZERO {
            &[] as &[u8]  // Empty slice for root storage
        } else {
            base_key.as_slice()
        };
        let slot = storage_key_map(storage_key, offset);
        tracing::info!(target: "arb-storage", "StorageBackedUint64::new base_key={:?} offset={} => slot={}", base_key, offset, slot);
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<u64, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache for in-flight changes (writes go to cache first)
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&self.slot) {
                        let value_u64: u64 = value.try_into().unwrap_or(0);
                        return Ok(value_u64);
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&self.slot) {
                    let value_u64: u64 = slot_entry.present_value.try_into().unwrap_or(0);
                    return Ok(value_u64);
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, self.slot) {
                Ok(value) => {
                    let value_u64: u64 = value.try_into().unwrap_or(0);
                    Ok(value_u64)
                }
                Err(_) => Err(()),
            }
        }
    }

    pub fn set(&self, value: u64) -> Result<(), ()> {
        let value_u256 = U256::from(value);
        unsafe {
            let state = &mut *self.storage;
            write_arbos_storage(state, self.slot, value_u256);
            Ok(())
        }
    }
}

pub struct StorageBackedBigUint<D> {
    storage: *mut revm::database::State<D>,
    slot: U256,
}

impl<D: Database> StorageBackedBigUint<D> {
    pub fn new(storage: *mut revm::database::State<D>, base_key: B256, offset: u64) -> Self {
        // Use the same keccak256-based slot mapping as Go nitro's Storage.NewSlot
        // IMPORTANT: Go nitro uses an empty []byte{} for root storage, not 32 zero bytes!
        let storage_key = if base_key == B256::ZERO {
            &[] as &[u8]  // Empty slice for root storage
        } else {
            base_key.as_slice()
        };
        let slot = storage_key_map(storage_key, offset);
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<U256, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache for in-flight changes (writes go to cache first)
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&self.slot) {
                        return Ok(value);
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&self.slot) {
                    return Ok(slot_entry.present_value);
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, self.slot) {
                Ok(value) => Ok(value),
                Err(_) => Err(()),
            }
        }
    }

    pub fn set(&self, value: U256) -> Result<(), ()> {
        unsafe {
            let state = &mut *self.storage;
            write_arbos_storage(state, self.slot, value);
            Ok(())
        }
    }
}

pub struct StorageBackedAddress<D> {
    storage: *mut revm::database::State<D>,
    slot: U256,
}

impl<D: Database> StorageBackedAddress<D> {
    pub fn new(storage: *mut revm::database::State<D>, base_key: B256, offset: u64) -> Self {
        // Use the same keccak256-based slot mapping as Go nitro's Storage.NewSlot
        // IMPORTANT: Go nitro uses an empty []byte{} for root storage, not 32 zero bytes!
        // When base_key is B256::ZERO, we should use an empty slice.
        let storage_key = if base_key == B256::ZERO {
            &[] as &[u8]  // Empty slice for root storage
        } else {
            base_key.as_slice()
        };
        let slot = storage_key_map(storage_key, offset);
        tracing::warn!(target: "arb-storage", "[ITER118] StorageBackedAddress::new base_key={:?} offset={} -> slot={:?}", base_key, offset, slot);
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<Address, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache for in-flight changes (writes go to cache first)
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&self.slot) {
                        let bytes = value.to_be_bytes::<32>();
                        let addr_bytes: [u8; 20] = bytes[12..32].try_into().unwrap();
                        return Ok(Address::from(addr_bytes));
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&self.slot) {
                    let bytes = slot_entry.present_value.to_be_bytes::<32>();
                    let addr_bytes: [u8; 20] = bytes[12..32].try_into().unwrap();
                    return Ok(Address::from(addr_bytes));
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, self.slot) {
                Ok(value) => {
                    let bytes = value.to_be_bytes::<32>();
                    let addr_bytes: [u8; 20] = bytes[12..32].try_into().unwrap();
                    Ok(Address::from(addr_bytes))
                }
                Err(_) => Err(()),
            }
        }
    }

    pub fn set(&self, value: Address) -> Result<(), ()> {
        let mut value_bytes = [0u8; 32];
        value_bytes[12..32].copy_from_slice(value.as_slice());
        let value_u256 = U256::from_be_bytes(value_bytes);
        unsafe {
            let state = &mut *self.storage;
            write_arbos_storage(state, self.slot, value_u256);
            Ok(())
        }
    }
}

/// StorageBackedInt64 matches Go nitro's StorageBackedInt64.
/// Go nitro stores int64 by casting to uint64 first (reinterpreting the bits),
/// then storing as a 256-bit value. This preserves the sign bit correctly.
pub struct StorageBackedInt64<D> {
    storage: *mut revm::database::State<D>,
    slot: U256,
}

impl<D: Database> StorageBackedInt64<D> {
    pub fn new(storage: *mut revm::database::State<D>, base_key: B256, offset: u64) -> Self {
        let storage_key = if base_key == B256::ZERO {
            &[] as &[u8]
        } else {
            base_key.as_slice()
        };
        let slot = storage_key_map(storage_key, offset);
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<i64, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache for in-flight changes
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&self.slot) {
                        // Go nitro casts uint64 to int64, reinterpreting the bits
                        let value_u64: u64 = value.try_into().unwrap_or(0);
                        return Ok(value_u64 as i64);
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&self.slot) {
                    let value_u64: u64 = slot_entry.present_value.try_into().unwrap_or(0);
                    return Ok(value_u64 as i64);
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, self.slot) {
                Ok(value) => {
                    let value_u64: u64 = value.try_into().unwrap_or(0);
                    Ok(value_u64 as i64)
                }
                Err(_) => Err(()),
            }
        }
    }

    pub fn set(&self, value: i64) -> Result<(), ()> {
        // Go nitro casts int64 to uint64, reinterpreting the bits
        let value_u256 = U256::from(value as u64);
        unsafe {
            let state = &mut *self.storage;
            write_arbos_storage(state, self.slot, value_u256);
            Ok(())
        }
    }
}

/// StorageBackedBigInt matches Go nitro's StorageBackedBigInt.
/// This stores signed 256-bit integers using two's complement representation.
/// Go nitro uses big.Int which can be negative, and stores it as a 256-bit value.
pub struct StorageBackedBigInt<D> {
    pub storage: *mut revm::database::State<D>,
    slot: U256,
}

impl<D: Database> StorageBackedBigInt<D> {
    pub fn new(storage: *mut revm::database::State<D>, base_key: B256, offset: u64) -> Self {
        let storage_key = if base_key == B256::ZERO {
            &[] as &[u8]
        } else {
            base_key.as_slice()
        };
        let slot = storage_key_map(storage_key, offset);
        Self { storage, slot }
    }

    /// Get the value as a signed 256-bit integer.
    /// Returns (value, is_negative) where value is the absolute magnitude as U256.
    /// For simplicity, we return the raw U256 and let the caller interpret it.
    pub fn get_raw(&self) -> Result<U256, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache for in-flight changes
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&self.slot) {
                        return Ok(value);
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&self.slot) {
                    return Ok(slot_entry.present_value);
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, self.slot) {
                Ok(value) => Ok(value),
                Err(_) => Err(()),
            }
        }
    }

    /// Check if the stored value is negative (high bit set in two's complement)
    pub fn is_negative(&self) -> Result<bool, ()> {
        let raw = self.get_raw()?;
        // Check if the high bit (bit 255) is set
        Ok(raw.bit(255))
    }

    /// Get the signed value as (magnitude, is_negative).
    /// This properly decodes two's complement representation.
    /// For negative values stored as 2^256 - magnitude, this returns (magnitude, true).
    /// For positive values, this returns (value, false).
    pub fn get_signed(&self) -> Result<(U256, bool), ()> {
        let raw = self.get_raw()?;
        if raw.bit(255) {
            // Negative: convert from two's complement
            // magnitude = 2^256 - raw = ~raw + 1
            let magnitude = (!raw).wrapping_add(U256::from(1));
            Ok((magnitude, true))
        } else {
            // Positive: raw is the magnitude
            Ok((raw, false))
        }
    }

    /// Set the value. For negative values, use set_negative.
    pub fn set(&self, value: U256) -> Result<(), ()> {
        unsafe {
            let state = &mut *self.storage;
            write_arbos_storage(state, self.slot, value);
            Ok(())
        }
    }

    /// Set a negative value using two's complement.
    /// The magnitude should be the absolute value.
    pub fn set_negative(&self, magnitude: U256) -> Result<(), ()> {
        // Two's complement: -x = ~x + 1 = (2^256 - 1 - x) + 1 = 2^256 - x
        // Since U256 wraps, we can compute this as: U256::MAX - magnitude + 1
        // Or equivalently: (!magnitude).wrapping_add(U256::from(1))
        let neg_value = (!magnitude).wrapping_add(U256::from(1));
        self.set(neg_value)
    }
}

/// StorageBackedAddressOrNil matches Go nitro's StorageBackedAddressOrNil.
/// It uses a special sentinel value (1 << 255) to represent nil addresses.
/// This is used for the retryable `to` field which can be nil for contract creation.
pub struct StorageBackedAddressOrNil<D> {
    storage: *mut revm::database::State<D>,
    slot: U256,
}

/// Go nitro's NilAddressRepresentation = common.BigToHash(new(big.Int).Lsh(big.NewInt(1), 255))
/// This is 1 << 255 = 0x8000000000000000000000000000000000000000000000000000000000000000
fn nil_address_representation() -> U256 {
    U256::from(1u64) << 255
}

impl<D: Database> StorageBackedAddressOrNil<D> {
    pub fn new(storage: *mut revm::database::State<D>, base_key: B256, offset: u64) -> Self {
        let storage_key = if base_key == B256::ZERO {
            &[] as &[u8]
        } else {
            base_key.as_slice()
        };
        let slot = storage_key_map(storage_key, offset);
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<Option<Address>, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = ARBOS_STATE_ADDRESS;
            let nil_repr = nil_address_representation();

            // First check cache for in-flight changes
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&value) = account.storage.get(&self.slot) {
                        if value == nil_repr {
                            return Ok(None);
                        }
                        let bytes = value.to_be_bytes::<32>();
                        let addr_bytes: [u8; 20] = bytes[12..32].try_into().unwrap();
                        return Ok(Some(Address::from(addr_bytes)));
                    }
                }
            }

            // Then check bundle_state.state for merged changes
            if let Some(acc) = state.bundle_state.state.get(&arbos_addr) {
                if let Some(slot_entry) = acc.storage.get(&self.slot) {
                    if slot_entry.present_value == nil_repr {
                        return Ok(None);
                    }
                    let bytes = slot_entry.present_value.to_be_bytes::<32>();
                    let addr_bytes: [u8; 20] = bytes[12..32].try_into().unwrap();
                    return Ok(Some(Address::from(addr_bytes)));
                }
            }

            // Fall back to database
            match state.database.storage(arbos_addr, self.slot) {
                Ok(value) => {
                    if value == nil_repr {
                        return Ok(None);
                    }
                    let bytes = value.to_be_bytes::<32>();
                    let addr_bytes: [u8; 20] = bytes[12..32].try_into().unwrap();
                    Ok(Some(Address::from(addr_bytes)))
                }
                Err(_) => Err(()),
            }
        }
    }

    /// Set an address or nil. If value is None, stores the nil sentinel value.
    /// If value is Some(addr), stores the address using BytesToHash encoding (right-aligned).
    pub fn set(&self, value: Option<Address>) -> Result<(), ()> {
        let value_u256 = match value {
            None => nil_address_representation(),
            Some(addr) => {
                // Go nitro uses common.BytesToHash(val.Bytes()) which right-aligns the address
                let mut value_bytes = [0u8; 32];
                value_bytes[12..32].copy_from_slice(addr.as_slice());
                U256::from_be_bytes(value_bytes)
            }
        };
        unsafe {
            let state = &mut *self.storage;
            write_arbos_storage(state, self.slot, value_u256);
            Ok(())
        }
    }
}
