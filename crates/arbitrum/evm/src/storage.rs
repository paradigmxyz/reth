use alloy_primitives::{Address, B256, U256};
use revm::Database;
use std::collections::HashMap;


pub struct Storage<D> {
    pub state: *mut revm::database::State<D>,
    pub base_key: B256,
}

impl<D: Database> Storage<D> {
    pub fn new(state: *mut revm::database::State<D>, base_key: B256) -> Self {
        Self { state, base_key }
    }

    pub fn open_sub_storage(&self, sub_key: &[u8]) -> Storage<D> {
        let mut combined = Vec::with_capacity(self.base_key.len() + sub_key.len());
        combined.extend_from_slice(self.base_key.as_slice());
        combined.extend_from_slice(sub_key);
        let new_key = alloy_primitives::keccak256(&combined);
        Storage::new(self.state, new_key)
    }

    pub fn get_by_uint64(&self, offset: u64) -> Result<B256, ()> {
        let slot = self.compute_slot(offset);
        unsafe {
            let state = &mut *self.state;
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
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
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
            let value_u256 = U256::from_be_bytes(value.0);
            
            use revm_state::EvmStorageSlot;
            use revm_database::{BundleAccount, AccountStatus};
            use std::collections::HashMap;
            
            if let Some(acc) = state.bundle_state.state.get_mut(&arbos_addr) {
                acc.storage.insert(
                    slot,
                    EvmStorageSlot { present_value: value_u256, ..Default::default() }.into(),
                );
            } else {
                let mut storage = HashMap::default();
                storage.insert(
                    slot,
                    EvmStorageSlot { present_value: value_u256, ..Default::default() }.into(),
                );
                let acc = BundleAccount {
                    info: None,
                    storage,
                    original_info: Default::default(),
                    status: AccountStatus::Changed,
                };
                state.bundle_state.state.insert(arbos_addr, acc);
            }
            Ok(())
        }
    }

    fn compute_slot(&self, offset: u64) -> U256 {
        let mut slot_bytes = [0u8; 32];
        slot_bytes[..32].copy_from_slice(self.base_key.as_slice());
        let offset_u256 = U256::from(offset);
        let base_slot = U256::from_be_bytes(slot_bytes);
        base_slot.wrapping_add(offset_u256)
    }
}

pub struct StorageBackedUint64<D> {
    storage: *mut revm::database::State<D>,
    slot: U256,
}

impl<D: Database> StorageBackedUint64<D> {
    pub fn new(storage: *mut revm::database::State<D>, base_key: B256, offset: u64) -> Self {
        let mut slot_bytes = [0u8; 32];
        slot_bytes[..32].copy_from_slice(base_key.as_slice());
        let base_slot = U256::from_be_bytes(slot_bytes);
        let slot = base_slot.wrapping_add(U256::from(offset));
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<u64, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
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
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
            let value_u256 = U256::from(value);
            
            use revm_state::EvmStorageSlot;
            use revm_database::{BundleAccount, AccountStatus};
            use std::collections::HashMap;
            
            if let Some(acc) = state.bundle_state.state.get_mut(&arbos_addr) {
                acc.storage.insert(
                    self.slot,
                    EvmStorageSlot { present_value: value_u256, ..Default::default() }.into(),
                );
            } else {
                let mut storage = HashMap::default();
                storage.insert(
                    self.slot,
                    EvmStorageSlot { present_value: value_u256, ..Default::default() }.into(),
                );
                let acc = BundleAccount {
                    info: None,
                    storage,
                    original_info: Default::default(),
                    status: AccountStatus::Changed,
                };
                state.bundle_state.state.insert(arbos_addr, acc);
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
        let mut slot_bytes = [0u8; 32];
        slot_bytes[..32].copy_from_slice(base_key.as_slice());
        let base_slot = U256::from_be_bytes(slot_bytes);
        let slot = base_slot.wrapping_add(U256::from(offset));
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<U256, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
            match state.storage(arbos_addr, self.slot) {
                Ok(value) => Ok(value),
                Err(_) => Err(()),
            }
        }
    }

    pub fn set(&self, value: U256) -> Result<(), ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
            
            use revm_state::EvmStorageSlot;
            use revm_database::{BundleAccount, AccountStatus};
            use std::collections::HashMap;
            
            if let Some(acc) = state.bundle_state.state.get_mut(&arbos_addr) {
                acc.storage.insert(
                    self.slot,
                    EvmStorageSlot { present_value: value, ..Default::default() }.into(),
                );
            } else {
                let mut storage = HashMap::default();
                storage.insert(
                    self.slot,
                    EvmStorageSlot { present_value: value, ..Default::default() }.into(),
                );
                let acc = BundleAccount {
                    info: None,
                    storage,
                    original_info: Default::default(),
                    status: AccountStatus::Changed,
                };
                state.bundle_state.state.insert(arbos_addr, acc);
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
        let mut slot_bytes = [0u8; 32];
        slot_bytes[..32].copy_from_slice(base_key.as_slice());
        let base_slot = U256::from_be_bytes(slot_bytes);
        let slot = base_slot.wrapping_add(U256::from(offset));
        Self { storage, slot }
    }

    pub fn get(&self) -> Result<Address, ()> {
        unsafe {
            let state = &mut *self.storage;
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
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
            let arbos_addr = Address::from([0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                           0x00, 0x00, 0x00, 0x64]);
            let mut value_bytes = [0u8; 32];
            value_bytes[12..32].copy_from_slice(value.as_slice());
            let value_u256 = U256::from_be_bytes(value_bytes);
            
            use revm_state::EvmStorageSlot;
            use revm_database::{BundleAccount, AccountStatus};
            use std::collections::HashMap;
            
            if let Some(acc) = state.bundle_state.state.get_mut(&arbos_addr) {
                acc.storage.insert(
                    self.slot,
                    EvmStorageSlot { present_value: value_u256, ..Default::default() }.into(),
                );
            } else {
                let mut storage = HashMap::default();
                storage.insert(
                    self.slot,
                    EvmStorageSlot { present_value: value_u256, ..Default::default() }.into(),
                );
                let acc = BundleAccount {
                    info: None,
                    storage,
                    original_info: Default::default(),
                    status: AccountStatus::Changed,
                };
                state.bundle_state.state.insert(arbos_addr, acc);
            }
            Ok(())
        }
    }
}
