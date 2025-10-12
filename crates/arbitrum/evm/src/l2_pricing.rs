use alloy_primitives::{Address, B256, U256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedBigUint, StorageBackedAddress};

pub struct L2PricingState<D> {
    storage: Storage<D>,
    
    gas_pool: StorageBackedBigUint<D>,
    pricing_inertia: StorageBackedUint64<D>,
    base_fee_l2: StorageBackedBigUint<D>,
    
    per_block_gas_limit: StorageBackedUint64<D>,
    speed_limit_per_second: StorageBackedUint64<D>,
    
    min_base_fee_wei: StorageBackedBigUint<D>,
}

const GAS_POOL_OFFSET: u64 = 0;
const PRICING_INERTIA_OFFSET: u64 = 1;
const BASE_FEE_L2_OFFSET: u64 = 2;
const PER_BLOCK_GAS_LIMIT_OFFSET: u64 = 3;
const SPEED_LIMIT_PER_SECOND_OFFSET: u64 = 4;
const MIN_BASE_FEE_WEI_OFFSET: u64 = 5;

const DEFAULT_PRICING_INERTIA: u64 = 10;
const DEFAULT_PER_BLOCK_GAS_LIMIT: u64 = 32_000_000;
const DEFAULT_SPEED_LIMIT_PER_SECOND: u64 = 7_000_000;
const DEFAULT_MIN_BASE_FEE_WEI: u128 = 100_000_000; // 0.1 Gwei

impl<D: Database> L2PricingState<D> {
    pub fn open(storage: Storage<D>) -> Self {
        let state = storage.state;
        let base_key = storage.base_key;
        
        Self {
            gas_pool: StorageBackedBigUint::new(state, base_key, GAS_POOL_OFFSET),
            pricing_inertia: StorageBackedUint64::new(state, base_key, PRICING_INERTIA_OFFSET),
            base_fee_l2: StorageBackedBigUint::new(state, base_key, BASE_FEE_L2_OFFSET),
            per_block_gas_limit: StorageBackedUint64::new(state, base_key, PER_BLOCK_GAS_LIMIT_OFFSET),
            speed_limit_per_second: StorageBackedUint64::new(state, base_key, SPEED_LIMIT_PER_SECOND_OFFSET),
            min_base_fee_wei: StorageBackedBigUint::new(state, base_key, MIN_BASE_FEE_WEI_OFFSET),
            storage,
        }
    }

    pub fn initialize(storage: &Storage<D>, initial_base_fee: U256) {
        let state = storage.state;
        let base_key = storage.base_key;
        
        let gas_pool = StorageBackedBigUint::new(state, base_key, GAS_POOL_OFFSET);
        let initial_pool = U256::from(DEFAULT_PER_BLOCK_GAS_LIMIT) * U256::from(2);
        gas_pool.set(initial_pool).ok();
        
        let pricing_inertia = StorageBackedUint64::new(state, base_key, PRICING_INERTIA_OFFSET);
        pricing_inertia.set(DEFAULT_PRICING_INERTIA).ok();
        
        let base_fee_l2 = StorageBackedBigUint::new(state, base_key, BASE_FEE_L2_OFFSET);
        base_fee_l2.set(initial_base_fee).ok();
        
        let per_block_gas_limit = StorageBackedUint64::new(state, base_key, PER_BLOCK_GAS_LIMIT_OFFSET);
        per_block_gas_limit.set(DEFAULT_PER_BLOCK_GAS_LIMIT).ok();
        
        let speed_limit_per_second = StorageBackedUint64::new(state, base_key, SPEED_LIMIT_PER_SECOND_OFFSET);
        speed_limit_per_second.set(DEFAULT_SPEED_LIMIT_PER_SECOND).ok();
        
        let min_base_fee_wei = StorageBackedBigUint::new(state, base_key, MIN_BASE_FEE_WEI_OFFSET);
        min_base_fee_wei.set(U256::from(DEFAULT_MIN_BASE_FEE_WEI)).ok();
    }

    pub fn set_initial_values(&self, initial_base_fee: U256) -> Result<(), ()> {
        let initial_pool = U256::from(DEFAULT_PER_BLOCK_GAS_LIMIT) * U256::from(2);
        self.gas_pool.set(initial_pool)?;
        
        self.pricing_inertia.set(DEFAULT_PRICING_INERTIA)?;
        self.base_fee_l2.set(initial_base_fee)?;
        self.per_block_gas_limit.set(DEFAULT_PER_BLOCK_GAS_LIMIT)?;
        self.speed_limit_per_second.set(DEFAULT_SPEED_LIMIT_PER_SECOND)?;
        self.min_base_fee_wei.set(U256::from(DEFAULT_MIN_BASE_FEE_WEI))?;
        
        Ok(())
    }

    pub fn get_gas_pool(&self) -> Result<U256, ()> {
        self.gas_pool.get()
    }

    pub fn set_gas_pool(&self, pool: U256) -> Result<(), ()> {
        self.gas_pool.set(pool)
    }

    pub fn get_base_fee_l2(&self) -> Result<U256, ()> {
        self.base_fee_l2.get()
    }

    pub fn set_base_fee_l2(&self, base_fee: U256) -> Result<(), ()> {
        let min_fee = self.min_base_fee_wei.get().unwrap_or(U256::from(DEFAULT_MIN_BASE_FEE_WEI));
        let actual_fee = base_fee.max(min_fee);
        self.base_fee_l2.set(actual_fee)
    }

    pub fn get_pricing_inertia(&self) -> Result<u64, ()> {
        self.pricing_inertia.get()
    }

    pub fn set_pricing_inertia(&self, inertia: u64) -> Result<(), ()> {
        self.pricing_inertia.set(inertia)
    }

    pub fn get_per_block_gas_limit(&self) -> Result<u64, ()> {
        self.per_block_gas_limit.get()
    }

    pub fn set_per_block_gas_limit(&self, limit: u64) -> Result<(), ()> {
        self.per_block_gas_limit.set(limit)
    }

    pub fn get_speed_limit_per_second(&self) -> Result<u64, ()> {
        self.speed_limit_per_second.get()
    }

    pub fn set_speed_limit_per_second(&self, limit: u64) -> Result<(), ()> {
        self.speed_limit_per_second.set(limit)
    }

    pub fn get_min_base_fee_wei(&self) -> Result<U256, ()> {
        self.min_base_fee_wei.get()
    }

    pub fn set_min_base_fee_wei(&self, min_fee: U256) -> Result<(), ()> {
        self.min_base_fee_wei.set(min_fee)
    }

    pub fn update_for_new_block(
        &self,
        gas_used: u64,
        time_since_last_block: u64,
    ) -> Result<U256, ()> {
        let mut gas_pool = self.get_gas_pool()?;
        let per_block_limit = self.get_per_block_gas_limit()?;
        let speed_limit = self.get_speed_limit_per_second()?;
        let current_base_fee = self.get_base_fee_l2()?;
        let inertia = self.get_pricing_inertia()?;
        
        let replenish_amount = U256::from(speed_limit) * U256::from(time_since_last_block);
        gas_pool = gas_pool.saturating_add(replenish_amount);
        
        let max_pool = U256::from(per_block_limit) * U256::from(2);
        if gas_pool > max_pool {
            gas_pool = max_pool;
        }
        
        let gas_used_u256 = U256::from(gas_used);
        gas_pool = gas_pool.saturating_sub(gas_used_u256);
        
        let target_pool = U256::from(per_block_limit);
        let new_base_fee = if gas_pool < target_pool {
            let deficit = target_pool - gas_pool;
            let increase = deficit / U256::from(inertia);
            current_base_fee.saturating_add(increase)
        } else {
            let surplus = gas_pool - target_pool;
            let decrease = surplus / U256::from(inertia);
            current_base_fee.saturating_sub(decrease)
        };
        
        self.set_gas_pool(gas_pool)?;
        self.set_base_fee_l2(new_base_fee)?;
        
        Ok(new_base_fee)
    }

    pub fn has_gas_available(&self, gas_limit: u64) -> Result<bool, ()> {
        let gas_pool = self.get_gas_pool()?;
        Ok(gas_pool >= U256::from(gas_limit))
    }

    pub fn add_to_gas_pool(&self, amount: i64) -> Result<(), ()> {
        let current_pool = self.get_gas_pool()?;
        
        let new_pool = if amount >= 0 {
            current_pool.saturating_add(U256::from(amount as u64))
        } else {
            let to_subtract = U256::from(amount.unsigned_abs());
            current_pool.saturating_sub(to_subtract)
        };
        
        self.set_gas_pool(new_pool)
    }
}
