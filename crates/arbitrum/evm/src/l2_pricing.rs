use alloy_primitives::{Address, B256, U256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedBigUint};

pub struct L2PricingState<D> {
    storage: Storage<D>,
    
    speed_limit_per_second: StorageBackedUint64<D>,  // offset 0
    per_block_gas_limit: StorageBackedUint64<D>,     // offset 1
    base_fee_wei: StorageBackedBigUint<D>,           // offset 2
    min_base_fee_wei: StorageBackedBigUint<D>,       // offset 3
    gas_backlog: StorageBackedUint64<D>,             // offset 4
    pricing_inertia: StorageBackedUint64<D>,         // offset 5
    backlog_tolerance: StorageBackedUint64<D>,       // offset 6
}

const SPEED_LIMIT_PER_SECOND_OFFSET: u64 = 0;
const PER_BLOCK_GAS_LIMIT_OFFSET: u64 = 1;
const BASE_FEE_WEI_OFFSET: u64 = 2;
const MIN_BASE_FEE_WEI_OFFSET: u64 = 3;
const GAS_BACKLOG_OFFSET: u64 = 4;
const PRICING_INERTIA_OFFSET: u64 = 5;
const BACKLOG_TOLERANCE_OFFSET: u64 = 6;

const INITIAL_SPEED_LIMIT_PER_SECOND_V6: u64 = 7_000_000;
const INITIAL_PER_BLOCK_GAS_LIMIT_V6: u64 = 32_000_000;
const INITIAL_MINIMUM_BASE_FEE_WEI: u128 = 100_000_000; // 0.1 Gwei
const INITIAL_BASE_FEE_WEI: u128 = INITIAL_MINIMUM_BASE_FEE_WEI;
const INITIAL_PRICING_INERTIA: u64 = 102;
const INITIAL_BACKLOG_TOLERANCE: u64 = 10;

impl<D: Database> L2PricingState<D> {
    pub fn open(storage: Storage<D>) -> Self {
        let state = storage.state;
        let base_key = storage.base_key;
        
        Self {
            speed_limit_per_second: StorageBackedUint64::new(state, base_key, SPEED_LIMIT_PER_SECOND_OFFSET),
            per_block_gas_limit: StorageBackedUint64::new(state, base_key, PER_BLOCK_GAS_LIMIT_OFFSET),
            base_fee_wei: StorageBackedBigUint::new(state, base_key, BASE_FEE_WEI_OFFSET),
            min_base_fee_wei: StorageBackedBigUint::new(state, base_key, MIN_BASE_FEE_WEI_OFFSET),
            gas_backlog: StorageBackedUint64::new(state, base_key, GAS_BACKLOG_OFFSET),
            pricing_inertia: StorageBackedUint64::new(state, base_key, PRICING_INERTIA_OFFSET),
            backlog_tolerance: StorageBackedUint64::new(state, base_key, BACKLOG_TOLERANCE_OFFSET),
            storage,
        }
    }

    pub fn initialize(storage: &Storage<D>, initial_base_fee: U256) {
        let state = storage.state;
        let base_key = storage.base_key;
        
        StorageBackedUint64::new(state, base_key, SPEED_LIMIT_PER_SECOND_OFFSET)
            .set(INITIAL_SPEED_LIMIT_PER_SECOND_V6).ok();
        
        StorageBackedUint64::new(state, base_key, PER_BLOCK_GAS_LIMIT_OFFSET)
            .set(INITIAL_PER_BLOCK_GAS_LIMIT_V6).ok();
        
        StorageBackedBigUint::new(state, base_key, BASE_FEE_WEI_OFFSET)
            .set(initial_base_fee).ok();
        
        StorageBackedUint64::new(state, base_key, GAS_BACKLOG_OFFSET)
            .set(0).ok();
        
        StorageBackedUint64::new(state, base_key, PRICING_INERTIA_OFFSET)
            .set(INITIAL_PRICING_INERTIA).ok();
        
        StorageBackedUint64::new(state, base_key, BACKLOG_TOLERANCE_OFFSET)
            .set(INITIAL_BACKLOG_TOLERANCE).ok();
        
        StorageBackedBigUint::new(state, base_key, MIN_BASE_FEE_WEI_OFFSET)
            .set(U256::from(INITIAL_MINIMUM_BASE_FEE_WEI)).ok();
    }


    pub fn base_fee_wei(&self) -> Result<U256, ()> {
        self.base_fee_wei.get()
    }

    pub fn set_base_fee_wei(&self, val: U256) -> Result<(), ()> {
        self.base_fee_wei.set(val)
    }

    pub fn min_base_fee_wei(&self) -> Result<U256, ()> {
        self.min_base_fee_wei.get()
    }

    pub fn set_min_base_fee_wei(&self, val: U256) -> Result<(), ()> {
        self.min_base_fee_wei.set(val)
    }

    pub fn speed_limit_per_second(&self) -> Result<u64, ()> {
        self.speed_limit_per_second.get()
    }

    pub fn set_speed_limit_per_second(&self, limit: u64) -> Result<(), ()> {
        self.speed_limit_per_second.set(limit)
    }

    pub fn per_block_gas_limit(&self) -> Result<u64, ()> {
        self.per_block_gas_limit.get()
    }

    pub fn set_per_block_gas_limit(&self, limit: u64) -> Result<(), ()> {
        self.per_block_gas_limit.set(limit)
    }

    pub fn gas_backlog(&self) -> Result<u64, ()> {
        self.gas_backlog.get()
    }

    pub fn set_gas_backlog(&self, backlog: u64) -> Result<(), ()> {
        self.gas_backlog.set(backlog)
    }

    pub fn pricing_inertia(&self) -> Result<u64, ()> {
        self.pricing_inertia.get()
    }

    pub fn set_pricing_inertia(&self, val: u64) -> Result<(), ()> {
        self.pricing_inertia.set(val)
    }

    pub fn backlog_tolerance(&self) -> Result<u64, ()> {
        self.backlog_tolerance.get()
    }

    pub fn set_backlog_tolerance(&self, val: u64) -> Result<(), ()> {
        self.backlog_tolerance.set(val)
    }

    pub fn add_to_gas_pool(&self, gas: i64) -> Result<(), ()> {
        let backlog = self.gas_backlog()?;
        
        let new_backlog = if gas > 0 {
            backlog.saturating_sub(gas as u64)
        } else {
            backlog.saturating_add((-gas) as u64)
        };
        
        self.set_gas_backlog(new_backlog)
    }
    
    pub fn update_pricing_model(&self, _l2_base_fee: U256, time_passed: u64) -> Result<(), ()> {
        let speed_limit = self.speed_limit_per_second()?;
        
        let gas_to_add = (time_passed as u128).saturating_mul(speed_limit as u128);
        let gas_to_add_i64 = if gas_to_add > i64::MAX as u128 {
            i64::MAX
        } else {
            gas_to_add as i64
        };
        
        self.add_to_gas_pool(gas_to_add_i64)?;
        
        let inertia = self.pricing_inertia()?;
        let tolerance = self.backlog_tolerance()?;
        let backlog = self.gas_backlog()?;
        let min_base_fee = self.min_base_fee_wei()?;
        
        let mut base_fee = min_base_fee;
        
        let tolerance_limit = tolerance.saturating_mul(speed_limit);
        if backlog > tolerance_limit {
            let excess = backlog.saturating_sub(tolerance_limit);
            
            let excess_bips = (excess as u128 * 10000) / (inertia as u128 * speed_limit as u128);
            
            let exp_result = Self::approx_exp_basis_points(excess_bips);
            
            base_fee = (min_base_fee * U256::from(exp_result)) / U256::from(10000u64);
        }
        
        self.set_base_fee_wei(base_fee)?;
        
        Ok(())
    }
    
    fn approx_exp_basis_points(bips: u128) -> u128 {
        if bips >= 63000 {
            return u128::MAX;
        }
        
        let mut result = 10000u128;
        let mut term = 10000u128;
        
        for i in 1..=4 {
            term = term.saturating_mul(bips) / (i * 10000);
            result = result.saturating_add(term);
            if term < 1 {
                break;
            }
        }
        
        result
    }
}

#[allow(dead_code)]
impl<D: Database> L2PricingState<D> {
    pub fn get_base_fee_l2(&self) -> Result<U256, ()> {
        self.base_fee_wei()
    }

    pub fn set_base_fee_l2(&self, base_fee: U256) -> Result<(), ()> {
        self.set_base_fee_wei(base_fee)
    }

    pub fn get_per_block_gas_limit(&self) -> Result<u64, ()> {
        self.per_block_gas_limit()
    }

    pub fn get_speed_limit_per_second(&self) -> Result<u64, ()> {
        self.speed_limit_per_second()
    }

    pub fn get_gas_backlog(&self) -> Result<u64, ()> {
        self.gas_backlog()
    }

    pub fn get_pricing_inertia(&self) -> Result<u64, ()> {
        self.pricing_inertia()
    }

    pub fn get_min_base_fee_wei(&self) -> Result<U256, ()> {
        self.min_base_fee_wei()
    }

    pub fn get_backlog_tolerance(&self) -> Result<u64, ()> {
        self.backlog_tolerance()
    }
}
