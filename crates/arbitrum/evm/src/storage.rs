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
/// CRITICAL FOR DETERMINISM: We NEVER call state.basic() because that creates cache entries
/// which can cause non-determinism between assembly and validation. However, we MUST read
/// the original account info from the database to preserve the genesis nonce (which is 1).
///
/// IMPORTANT: We use AccountStatus::Loaded (not Changed) because we're not modifying the
/// account info itself - only the storage. The account already exists in the trie from genesis.
fn ensure_arbos_account_in_bundle<D: Database>(state: &mut revm::database::State<D>) {
    use revm_database::{BundleAccount, AccountStatus};
    use revm_state::AccountInfo;

    if !state.bundle_state.state.contains_key(&ARBOS_STATE_ADDRESS) {
        // Read original info from database to preserve genesis nonce (which is 1)
        // We read directly from database, not from state.basic() which would create cache entries
        let original_info = state.database.basic(ARBOS_STATE_ADDRESS)
            .ok()
            .flatten()
            .unwrap_or_else(|| AccountInfo {
                balance: U256::ZERO,
                nonce: 1,  // Default to 1 if not found (matches genesis)
                code_hash: alloy_primitives::keccak256([]),
                code: None,
            });

        let info = Some(AccountInfo {
            balance: original_info.balance,
            nonce: original_info.nonce,
            code_hash: original_info.code_hash,
            code: original_info.code.clone(),
        });

        // Use Loaded status because this account already exists in the trie from genesis.
        // We're only modifying storage, not the account info itself.
        let acc = BundleAccount {
            info,
            storage: HashMap::default(),
            original_info: Some(original_info),
            status: AccountStatus::Loaded,
        };
        state.bundle_state.state.insert(ARBOS_STATE_ADDRESS, acc);
    }
}

/// Helper function to write storage for the ArbOS account.
///
/// This writes DIRECTLY to bundle_state.state which is the approach that produces
/// DETERMINISTIC state roots. The cache/transition mechanism was causing non-determinism
/// because transitions interact with EVM execution in complex ways.
///
/// bundle_state.state persists throughout block execution and is used directly
/// when computing the state root, so writes here are guaranteed to be included.
fn write_arbos_storage<D: Database>(
    state: &mut revm::database::State<D>,
    slot: U256,
    value: U256,
) {
    use revm_state::EvmStorageSlot;

    ensure_arbos_account_in_bundle(state);

    // Get original value for proper tracking
    let original_value = if let Some(acc) = state.bundle_state.state.get(&ARBOS_STATE_ADDRESS) {
        if let Some(slot_entry) = acc.storage.get(&slot) {
            slot_entry.previous_or_original_value
        } else {
            state.database.storage(ARBOS_STATE_ADDRESS, slot).unwrap_or(U256::ZERO)
        }
    } else {
        state.database.storage(ARBOS_STATE_ADDRESS, slot).unwrap_or(U256::ZERO)
    };

    // Write directly to bundle_state.state
    if let Some(acc) = state.bundle_state.state.get_mut(&ARBOS_STATE_ADDRESS) {
        acc.storage.insert(
            slot,
            EvmStorageSlot::new_changed(original_value, value, 0).into(),
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

            // First check bundle_state.state for any in-flight changes
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

    pub fn get(&self, key: B256) -> Result<B256, ()> {
        unsafe {
            let state = &mut *self.state;
            let arbos_addr = ARBOS_STATE_ADDRESS;
            let slot = U256::from_be_bytes(key.0);

            // First check bundle_state.state for any in-flight changes
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
        let slot = U256::from_be_bytes(key.0);
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

            // First check bundle_state.state for any in-flight changes
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

            // First check bundle_state.state for any in-flight changes
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

            // First check bundle_state.state for any in-flight changes
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
