#![allow(unused)]

use alloy_primitives::{Address, U256, B256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64};

pub struct AddressTable<D> {
    backing_storage: Storage<D>,
    by_address: Storage<D>,
    num_items: StorageBackedUint64<D>,
}

impl<D: Database> AddressTable<D> {
    pub fn initialize(_sto: &Storage<D>) {
    }

    pub fn open(sto: Storage<D>) -> Self {
        let num_items = StorageBackedUint64::new(sto.state, sto.base_key, 0);
        let by_address = sto.open_sub_storage(&[]);
        
        Self {
            backing_storage: sto.clone(),
            by_address,
            num_items,
        }
    }

    pub fn register(&self, addr: Address) -> Result<u64, ()> {
        let addr_hash = address_to_hash(addr);
        let rev = self.by_address.get(addr_hash)?;
        
        if rev != B256::ZERO {
            return Ok(U256::from_be_bytes(rev.0).to::<u64>() - 1);
        }
        
        let current = self.num_items.get()?;
        let new_num_items = current + 1;
        self.num_items.set(new_num_items)?;
        
        self.backing_storage.set_by_uint64(new_num_items, addr_hash)?;
        self.by_address.set(addr_hash, uint_to_hash(new_num_items))?;
        
        Ok(new_num_items - 1)
    }

    pub fn lookup(&self, addr: Address) -> Result<(u64, bool), ()> {
        let addr_hash = address_to_hash(addr);
        let res_hash = self.by_address.get(addr_hash)?;
        let res = U256::from_be_bytes(res_hash.0).to::<u64>();
        
        if res == 0 {
            Ok((0, false))
        } else {
            Ok((res - 1, true))
        }
    }

    pub fn address_exists(&self, addr: Address) -> Result<bool, ()> {
        let (_, exists) = self.lookup(addr)?;
        Ok(exists)
    }

    pub fn size(&self) -> Result<u64, ()> {
        self.num_items.get()
    }

    pub fn lookup_index(&self, index: u64) -> Result<Option<Address>, ()> {
        let items = self.num_items.get()?;
        if index >= items {
            return Ok(None);
        }
        
        let value = self.backing_storage.get_by_uint64(index + 1)?;
        let mut addr_bytes = [0u8; 20];
        addr_bytes.copy_from_slice(&value.0[12..32]);
        Ok(Some(Address::from(addr_bytes)))
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
