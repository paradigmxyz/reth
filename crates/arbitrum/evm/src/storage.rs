use alloy_primitives::{Address, B256, U256, keccak256, address};
use revm::Database;
use revm_database::states::plain_account::StorageSlot;

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

fn ensure_arbos_account_loaded<D: Database>(state: &mut revm::database::State<D>) {
    // Load the ArbOS account into the cache (if not already there)
    // This is the proper way to ensure the account is available for storage operations
    let _ = state.load_cache_account(ARBOS_STATE_ADDRESS);
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
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;
            match state.storage(arbos_addr, slot) {
                Ok(value) => Ok(B256::from(value)),
                Err(_) => Err(()),
            }
        }
    }

    pub fn set_by_uint64(&self, offset: u64, value: B256) -> Result<(), ()> {
        let slot = self.compute_slot(offset);
        unsafe {
            let state = &mut *self.state;
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;
            let value_u256 = U256::from_be_bytes(value.0);

            // Get original value from database for proper tracking
            let original_value = state.storage(arbos_addr, slot).unwrap_or(U256::ZERO);

            // Update cache.accounts with the storage change
            if let Some(cached_acc) = state.cache.accounts.get_mut(&arbos_addr) {
                let previous_status = cached_acc.status;
                let previous_info = cached_acc.account.as_ref().map(|a| a.info.clone());

                // Update the cached account's storage (PlainStorage = HashMap<U256, U256>)
                if let Some(ref mut account) = cached_acc.account {
                    account.storage.insert(slot, value_u256);
                }

                let had_no_nonce_and_code = previous_info
                    .as_ref()
                    .map(|info| info.has_no_code_and_nonce())
                    .unwrap_or_default();
                cached_acc.status = cached_acc.status.on_changed(had_no_nonce_and_code);

                // Create and apply the transition with storage change
                let mut storage_changes = alloy_primitives::map::HashMap::default();
                storage_changes.insert(slot, StorageSlot::new_changed(original_value, value_u256));

                let transition = revm::database::TransitionAccount {
                    info: cached_acc.account.as_ref().map(|a| a.info.clone()),
                    status: cached_acc.status,
                    previous_info,
                    previous_status,
                    storage: storage_changes,
                    storage_was_destroyed: false,
                };
                state.apply_transition(vec![(arbos_addr, transition)]);
            }
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

    pub fn get(&self, key: B256) -> Result<B256, ()> {
        unsafe {
            let state = &mut *self.state;
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;
            let slot = U256::from_be_bytes(key.0);
            match state.storage(arbos_addr, slot) {
                Ok(value) => Ok(B256::from(value)),
                Err(_) => Ok(B256::ZERO),
            }
        }
    }

    pub fn set(&self, key: B256, value: B256) -> Result<(), ()> {
        unsafe {
            let state = &mut *self.state;
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;
            let slot = U256::from_be_bytes(key.0);
            let value_u256 = U256::from_be_bytes(value.0);

            // Get original value from database for proper tracking
            let original_value = state.storage(arbos_addr, slot).unwrap_or(U256::ZERO);

            // Update cache.accounts with the storage change
            if let Some(cached_acc) = state.cache.accounts.get_mut(&arbos_addr) {
                let previous_status = cached_acc.status;
                let previous_info = cached_acc.account.as_ref().map(|a| a.info.clone());

                // Update the cached account's storage (PlainStorage = HashMap<U256, U256>)
                if let Some(ref mut account) = cached_acc.account {
                    account.storage.insert(slot, value_u256);
                }

                let had_no_nonce_and_code = previous_info
                    .as_ref()
                    .map(|info| info.has_no_code_and_nonce())
                    .unwrap_or_default();
                cached_acc.status = cached_acc.status.on_changed(had_no_nonce_and_code);

                // Create and apply the transition with storage change
                let mut storage_changes = alloy_primitives::map::HashMap::default();
                storage_changes.insert(slot, StorageSlot::new_changed(original_value, value_u256));

                let transition = revm::database::TransitionAccount {
                    info: cached_acc.account.as_ref().map(|a| a.info.clone()),
                    status: cached_acc.status,
                    previous_info,
                    previous_status,
                    storage: storage_changes,
                    storage_was_destroyed: false,
                };
                state.apply_transition(vec![(arbos_addr, transition)]);
            }
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
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache.accounts for any in-flight changes
            // Note: cache storage is PlainStorage = HashMap<U256, U256>, entries are plain U256
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&slot_value) = account.storage.get(&self.slot) {
                        let value_u64: u64 = slot_value.try_into().unwrap_or(0);
                        return Ok(value_u64);
                    }
                }
            }

            // Fall back to database
            match state.storage(arbos_addr, self.slot) {
                Ok(value) => {
                    let value_u64: u64 = value.try_into().unwrap_or(0);
                    Ok(value_u64)
                }
                Err(_) => Err(()),
            }
        }
    }

    pub fn set(&self, value: u64) -> Result<(), ()> {
        unsafe {
            let state = &mut *self.storage;
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;
            let value_u256 = U256::from(value);

            // Get original value from database for proper tracking
            let original_value = state.storage(arbos_addr, self.slot).unwrap_or(U256::ZERO);

            // Update cache.accounts with the storage change
            if let Some(cached_acc) = state.cache.accounts.get_mut(&arbos_addr) {
                let previous_status = cached_acc.status;
                let previous_info = cached_acc.account.as_ref().map(|a| a.info.clone());

                // Update the cached account's storage (PlainStorage = HashMap<U256, U256>)
                if let Some(ref mut account) = cached_acc.account {
                    account.storage.insert(self.slot, value_u256);
                }

                let had_no_nonce_and_code = previous_info
                    .as_ref()
                    .map(|info| info.has_no_code_and_nonce())
                    .unwrap_or_default();
                cached_acc.status = cached_acc.status.on_changed(had_no_nonce_and_code);

                // Create and apply the transition with storage change
                let mut storage_changes = alloy_primitives::map::HashMap::default();
                storage_changes.insert(self.slot, StorageSlot::new_changed(original_value, value_u256));

                let transition = revm::database::TransitionAccount {
                    info: cached_acc.account.as_ref().map(|a| a.info.clone()),
                    status: cached_acc.status,
                    previous_info,
                    previous_status,
                    storage: storage_changes,
                    storage_was_destroyed: false,
                };
                state.apply_transition(vec![(arbos_addr, transition)]);
            }
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
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache.accounts for any in-flight changes
            // Note: cache storage is PlainStorage = HashMap<U256, U256>, entries are plain U256
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&slot_value) = account.storage.get(&self.slot) {
                        return Ok(slot_value);
                    }
                }
            }

            // Fall back to database
            match state.storage(arbos_addr, self.slot) {
                Ok(value) => Ok(value),
                Err(_) => Err(()),
            }
        }
    }

    pub fn set(&self, value: U256) -> Result<(), ()> {
        unsafe {
            let state = &mut *self.storage;
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // Get original value from database for proper tracking
            let original_value = state.storage(arbos_addr, self.slot).unwrap_or(U256::ZERO);

            // Update cache.accounts with the storage change
            if let Some(cached_acc) = state.cache.accounts.get_mut(&arbos_addr) {
                let previous_status = cached_acc.status;
                let previous_info = cached_acc.account.as_ref().map(|a| a.info.clone());

                // Update the cached account's storage (PlainStorage = HashMap<U256, U256>)
                if let Some(ref mut account) = cached_acc.account {
                    account.storage.insert(self.slot, value);
                }

                let had_no_nonce_and_code = previous_info
                    .as_ref()
                    .map(|info| info.has_no_code_and_nonce())
                    .unwrap_or_default();
                cached_acc.status = cached_acc.status.on_changed(had_no_nonce_and_code);

                // Create and apply the transition with storage change
                let mut storage_changes = alloy_primitives::map::HashMap::default();
                storage_changes.insert(self.slot, StorageSlot::new_changed(original_value, value));

                let transition = revm::database::TransitionAccount {
                    info: cached_acc.account.as_ref().map(|a| a.info.clone()),
                    status: cached_acc.status,
                    previous_info,
                    previous_status,
                    storage: storage_changes,
                    storage_was_destroyed: false,
                };
                state.apply_transition(vec![(arbos_addr, transition)]);
            }
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
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;

            // First check cache.accounts for any in-flight changes
            // Note: cache storage is PlainStorage = HashMap<U256, U256>, entries are plain U256
            if let Some(cached_acc) = state.cache.accounts.get(&arbos_addr) {
                if let Some(ref account) = cached_acc.account {
                    if let Some(&slot_value) = account.storage.get(&self.slot) {
                        let bytes = slot_value.to_be_bytes::<32>();
                        let addr_bytes: [u8; 20] = bytes[12..32].try_into().unwrap();
                        return Ok(Address::from(addr_bytes));
                    }
                }
            }

            // Fall back to database
            match state.storage(arbos_addr, self.slot) {
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
        unsafe {
            let state = &mut *self.storage;
            ensure_arbos_account_loaded(state);
            let arbos_addr = ARBOS_STATE_ADDRESS;
            let mut value_bytes = [0u8; 32];
            value_bytes[12..32].copy_from_slice(value.as_slice());
            let value_u256 = U256::from_be_bytes(value_bytes);

            // Get original value from database for proper tracking
            let original_value = state.storage(arbos_addr, self.slot).unwrap_or(U256::ZERO);

            // Update cache.accounts with the storage change
            if let Some(cached_acc) = state.cache.accounts.get_mut(&arbos_addr) {
                let previous_status = cached_acc.status;
                let previous_info = cached_acc.account.as_ref().map(|a| a.info.clone());

                // Update the cached account's storage (PlainStorage = HashMap<U256, U256>)
                if let Some(ref mut account) = cached_acc.account {
                    account.storage.insert(self.slot, value_u256);
                }

                let had_no_nonce_and_code = previous_info
                    .as_ref()
                    .map(|info| info.has_no_code_and_nonce())
                    .unwrap_or_default();
                cached_acc.status = cached_acc.status.on_changed(had_no_nonce_and_code);

                // Create and apply the transition with storage change
                let mut storage_changes = alloy_primitives::map::HashMap::default();
                storage_changes.insert(self.slot, StorageSlot::new_changed(original_value, value_u256));

                let transition = revm::database::TransitionAccount {
                    info: cached_acc.account.as_ref().map(|a| a.info.clone()),
                    status: cached_acc.status,
                    previous_info,
                    previous_status,
                    storage: storage_changes,
                    storage_was_destroyed: false,
                };
                state.apply_transition(vec![(arbos_addr, transition)]);
            }
            Ok(())
        }
    }
}
