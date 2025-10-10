#![allow(unused)]

use alloy_primitives::U256;
use revm::Database;
use crate::storage::{Storage, StorageBackedBigUint};

const INCREASED_CALLDATA: u32 = 0;

pub struct Features<D> {
    features: StorageBackedBigUint<D>,
}

impl<D: Database> Features<D> {
    pub fn open(sto: Storage<D>) -> Self {
        let features = StorageBackedBigUint::new(sto.state, sto.base_key, 0);
        
        Self {
            features,
        }
    }

    pub fn set_calldata_price_increase(&self, enabled: bool) -> Result<(), ()> {
        self.set_bit(INCREASED_CALLDATA, enabled)
    }

    pub fn is_increased_calldata_price_enabled(&self) -> Result<bool, ()> {
        self.is_set(INCREASED_CALLDATA)
    }

    fn set_bit(&self, index: u32, enabled: bool) -> Result<(), ()> {
        let mut bi = self.features.get().unwrap_or(U256::ZERO);
        
        if enabled {
            bi |= U256::from(1) << index;
        } else {
            bi &= !(U256::from(1) << index);
        }
        
        self.features.set(bi)
    }

    fn is_set(&self, index: u32) -> Result<bool, ()> {
        let bi = self.features.get().unwrap_or(U256::ZERO);
        let mask = U256::from(1) << index;
        Ok((bi & mask) != U256::ZERO)
    }
}
