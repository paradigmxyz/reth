#![allow(unused)]

use alloy_primitives::{Address, U256, B256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedAddress};

pub struct AddressSet<D> {
    backing_storage: Storage<D>,
    size: StorageBackedUint64<D>,
    by_address: Storage<D>,
}

impl<D: Database> AddressSet<D> {
    pub fn initialize(sto: &Storage<D>) -> Result<(), ()> {
        sto.set_by_uint64(0, B256::ZERO)
    }

    pub fn open(sto: Storage<D>) -> Self {
        let size = StorageBackedUint64::new(sto.state, sto.base_key, 0);
        let by_address = sto.open_sub_storage(&[0u8]);
        
        Self {
            backing_storage: sto.clone(),
            size,
            by_address,
        }
    }

    pub fn size(&self) -> Result<u64, ()> {
        self.size.get()
    }

    pub fn is_member(&self, addr: Address) -> Result<bool, ()> {
        let addr_hash = address_to_hash(addr);
        let value = self.by_address.get(addr_hash)?;
        Ok(value != B256::ZERO)
    }

    pub fn get_any_member(&self) -> Result<Option<Address>, ()> {
        let size = self.size.get()?;
        if size == 0 {
            return Ok(None);
        }
        let sba = StorageBackedAddress::new(self.backing_storage.state, self.backing_storage.base_key, 1);
        sba.get().map(Some)
    }

    pub fn clear(&self) -> Result<(), ()> {
        let size = self.size.get()?;
        if size == 0 {
            return Ok(());
        }
        
        for i in 1..=size {
            let contents = self.backing_storage.get_by_uint64(i)?;
            self.backing_storage.set_by_uint64(i, B256::ZERO)?;
            self.by_address.set(contents, B256::ZERO)?;
        }
        
        self.size.set(0)
    }

    pub fn all_members(&self, max_num: u64) -> Result<Vec<Address>, ()> {
        let mut size = self.size.get()?;
        if size > max_num {
            size = max_num;
        }
        
        let mut ret = Vec::with_capacity(size as usize);
        for i in 0..size {
            let sba = StorageBackedAddress::new(self.backing_storage.state, self.backing_storage.base_key, i + 1);
            ret.push(sba.get()?);
        }
        
        Ok(ret)
    }

    pub fn clear_list(&self) -> Result<(), ()> {
        let size = self.size.get()?;
        if size == 0 {
            return Ok(());
        }
        
        for i in 1..=size {
            self.backing_storage.set_by_uint64(i, B256::ZERO)?;
        }
        
        self.size.set(0)
    }

    pub fn add(&self, addr: Address) -> Result<(), ()> {
        let present = self.is_member(addr)?;
        if present {
            return Ok(());
        }
        
        let size = self.size.get()?;
        let slot = uint_to_hash(1 + size);
        let addr_hash = address_to_hash(addr);
        
        self.by_address.set(addr_hash, slot)?;
        
        let sba = StorageBackedAddress::new(self.backing_storage.state, self.backing_storage.base_key, 1 + size);
        sba.set(addr)?;
        
        self.size.set(size + 1)
    }

    pub fn remove(&self, addr: Address, arbos_version: u64) -> Result<(), ()> {
        let addr_hash = address_to_hash(addr);
        let slot_hash = self.by_address.get(addr_hash)?;
        let slot = hash_to_uint64(slot_hash);
        
        if slot == 0 {
            return Ok(());
        }
        
        self.by_address.set(addr_hash, B256::ZERO)?;
        
        let size = self.size.get()?;
        if slot < size {
            let at_size = self.backing_storage.get_by_uint64(size)?;
            self.backing_storage.set_by_uint64(slot, at_size)?;
            
            if arbos_version >= 11 {
                self.by_address.set(at_size, uint_to_hash(slot))?;
            }
        }
        
        self.backing_storage.set_by_uint64(size, B256::ZERO)?;
        self.size.set(size - 1)
    }
}

fn address_to_hash(addr: Address) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[12..32].copy_from_slice(addr.as_slice());
    B256::from(bytes)
}

fn uint_to_hash(val: u64) -> B256 {
    B256::from(U256::from(val))
}

fn hash_to_uint64(hash: B256) -> u64 {
    U256::from_be_bytes(hash.0).to::<u64>()
}
