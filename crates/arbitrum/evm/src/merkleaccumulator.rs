#![allow(unused)]

use alloy_primitives::{keccak256, B256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64};

pub struct MerkleAccumulator<D> {
    backing_storage: Storage<D>,
    size: StorageBackedUint64<D>,
}

impl<D: Database> MerkleAccumulator<D> {
    pub fn initialize(_sto: &Storage<D>) {
    }

    pub fn open(sto: Storage<D>) -> Self {
        let size = StorageBackedUint64::new(sto.state, sto.base_key, 0);
        
        Self {
            backing_storage: sto,
            size,
        }
    }

    pub fn calc_num_partials(size: u64) -> u64 {
        if size == 0 {
            return 0;
        }
        64 - (size - 1).leading_zeros() as u64
    }

    fn get_partial(&self, level: u64) -> Result<B256, ()> {
        self.backing_storage.get_by_uint64(2 + level)
    }

    fn set_partial(&self, level: u64, val: B256) -> Result<(), ()> {
        self.backing_storage.set_by_uint64(2 + level, val)
    }

    pub fn append(&self, item_hash: B256) -> Result<(), ()> {
        let current_size = self.size.get()?;
        let new_size = current_size + 1;
        self.size.set(new_size)?;

        let mut level = 0u64;
        let mut so_far = keccak256(item_hash.as_slice());

        loop {
            if level == Self::calc_num_partials(current_size) {
                self.set_partial(level, so_far)?;
                return Ok(());
            }

            let this_level = self.get_partial(level)?;
            if this_level == B256::ZERO {
                self.set_partial(level, so_far)?;
                return Ok(());
            }

            let mut combined = Vec::with_capacity(64);
            combined.extend_from_slice(this_level.as_slice());
            combined.extend_from_slice(so_far.as_slice());
            so_far = keccak256(&combined);

            self.set_partial(level, B256::ZERO)?;
            
            level += 1;
        }
    }

    pub fn size(&self) -> Result<u64, ()> {
        self.size.get()
    }

    pub fn root(&self) -> Result<B256, ()> {
        let size = self.size.get()?;
        if size == 0 {
            return Ok(B256::ZERO);
        }

        let mut hash_so_far: Option<B256> = None;
        let mut capacity_in_hash = 0u64;
        let mut capacity = 1u64;

        for level in 0..Self::calc_num_partials(size) {
            let partial = self.get_partial(level)?;
            if partial != B256::ZERO {
                if let Some(ref mut current_hash) = hash_so_far {
                    while capacity_in_hash < capacity {
                        let mut combined = Vec::with_capacity(64);
                        combined.extend_from_slice(current_hash.as_slice());
                        combined.extend_from_slice(&[0u8; 32]);
                        *current_hash = keccak256(&combined);
                        capacity_in_hash *= 2;
                    }

                    let mut combined = Vec::with_capacity(64);
                    combined.extend_from_slice(partial.as_slice());
                    combined.extend_from_slice(current_hash.as_slice());
                    *current_hash = keccak256(&combined);
                    capacity_in_hash = 2 * capacity;
                } else {
                    hash_so_far = Some(partial);
                    capacity_in_hash = capacity;
                }
            }
            capacity *= 2;
        }

        Ok(hash_so_far.unwrap_or(B256::ZERO))
    }
}
