use alloy_primitives::{Address, B256, U256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedBigUint, StorageBackedAddress};

pub struct L1PricingState<D> {
    storage: Storage<D>,
    
    pay_rewards_to: StorageBackedAddress<D>,
    equilibration_units: StorageBackedBigUint<D>,
    inertia: StorageBackedUint64<D>,
    per_unit_reward: StorageBackedUint64<D>,
    
    last_update_time: StorageBackedUint64<D>,
    funds_due_for_rewards: StorageBackedBigUint<D>,
    units_since_update: StorageBackedUint64<D>,
    price_per_unit: StorageBackedBigUint<D>,
    last_surplus: StorageBackedBigUint<D>,
    per_batch_gas_cost: StorageBackedUint64<D>,
    amortized_cost_cap_bips: StorageBackedUint64<D>,
    l1_fees_available: StorageBackedBigUint<D>,
    gas_floor_per_token: StorageBackedUint64<D>,
    
    arbos_version: u64,
}

const PAY_REWARDS_TO_OFFSET: u64 = 0;
const EQUILIBRATION_UNITS_OFFSET: u64 = 1;
const INERTIA_OFFSET: u64 = 2;
const PER_UNIT_REWARD_OFFSET: u64 = 3;
const LAST_UPDATE_TIME_OFFSET: u64 = 4;
const FUNDS_DUE_FOR_REWARDS_OFFSET: u64 = 5;
const UNITS_SINCE_OFFSET: u64 = 6;
const PRICE_PER_UNIT_OFFSET: u64 = 7;
const LAST_SURPLUS_OFFSET: u64 = 8;
const PER_BATCH_GAS_COST_OFFSET: u64 = 9;
const AMORTIZED_COST_CAP_BIPS_OFFSET: u64 = 10;
const L1_FEES_AVAILABLE_OFFSET: u64 = 11;
const GAS_FLOOR_PER_TOKEN_OFFSET: u64 = 12;

const INITIAL_INERTIA: u64 = 10;
const INITIAL_PER_UNIT_REWARD: u64 = 10;
const INITIAL_PER_BATCH_GAS_COST_V6: u64 = 100_000;
const INITIAL_PER_BATCH_GAS_COST_V12: u64 = 210_000;

pub const BATCH_POSTER_ADDRESS: Address = Address::new([
    0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x73, 0x65, 0x71, 0x75, 0x65, 0x6e,
    0x63, 0x65, 0x72, 0x00
]);

pub const L1_PRICER_FUNDS_POOL_ADDRESS: Address = Address::new([
    0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0xf6
]);

impl<D: Database> L1PricingState<D> {
    pub fn open(storage: Storage<D>, arbos_version: u64) -> Self {
        let state = storage.state;
        let base_key = storage.base_key;
        
        Self {
            pay_rewards_to: StorageBackedAddress::new(state, base_key, PAY_REWARDS_TO_OFFSET),
            equilibration_units: StorageBackedBigUint::new(state, base_key, EQUILIBRATION_UNITS_OFFSET),
            inertia: StorageBackedUint64::new(state, base_key, INERTIA_OFFSET),
            per_unit_reward: StorageBackedUint64::new(state, base_key, PER_UNIT_REWARD_OFFSET),
            last_update_time: StorageBackedUint64::new(state, base_key, LAST_UPDATE_TIME_OFFSET),
            funds_due_for_rewards: StorageBackedBigUint::new(state, base_key, FUNDS_DUE_FOR_REWARDS_OFFSET),
            units_since_update: StorageBackedUint64::new(state, base_key, UNITS_SINCE_OFFSET),
            price_per_unit: StorageBackedBigUint::new(state, base_key, PRICE_PER_UNIT_OFFSET),
            last_surplus: StorageBackedBigUint::new(state, base_key, LAST_SURPLUS_OFFSET),
            per_batch_gas_cost: StorageBackedUint64::new(state, base_key, PER_BATCH_GAS_COST_OFFSET),
            amortized_cost_cap_bips: StorageBackedUint64::new(state, base_key, AMORTIZED_COST_CAP_BIPS_OFFSET),
            l1_fees_available: StorageBackedBigUint::new(state, base_key, L1_FEES_AVAILABLE_OFFSET),
            gas_floor_per_token: StorageBackedUint64::new(state, base_key, GAS_FLOOR_PER_TOKEN_OFFSET),
            storage,
            arbos_version,
        }
    }

    pub fn initialize(
        storage: &Storage<D>,
        initial_rewards_recipient: Address,
        initial_l1_base_fee: U256,
    ) {
        let state = storage.state;
        let base_key = storage.base_key;
        
        let pay_rewards_to = StorageBackedAddress::new(state, base_key, PAY_REWARDS_TO_OFFSET);
        pay_rewards_to.set(initial_rewards_recipient).ok();
        
        let equilibration_units = StorageBackedBigUint::new(state, base_key, EQUILIBRATION_UNITS_OFFSET);
        let initial_equilibration_units = U256::from(60u64 * 16u64 * 100_000u64);
        equilibration_units.set(initial_equilibration_units).ok();
        
        let inertia = StorageBackedUint64::new(state, base_key, INERTIA_OFFSET);
        inertia.set(INITIAL_INERTIA).ok();
        
        let per_unit_reward = StorageBackedUint64::new(state, base_key, PER_UNIT_REWARD_OFFSET);
        per_unit_reward.set(INITIAL_PER_UNIT_REWARD).ok();
        
        let funds_due_for_rewards = StorageBackedBigUint::new(state, base_key, FUNDS_DUE_FOR_REWARDS_OFFSET);
        funds_due_for_rewards.set(U256::ZERO).ok();
        
        let price_per_unit = StorageBackedBigUint::new(state, base_key, PRICE_PER_UNIT_OFFSET);
        price_per_unit.set(initial_l1_base_fee).ok();
        
        let last_update_time = StorageBackedUint64::new(state, base_key, LAST_UPDATE_TIME_OFFSET);
        last_update_time.set(0).ok();
        
        let units_since_update = StorageBackedUint64::new(state, base_key, UNITS_SINCE_OFFSET);
        units_since_update.set(0).ok();
        
        let last_surplus = StorageBackedBigUint::new(state, base_key, LAST_SURPLUS_OFFSET);
        last_surplus.set(U256::ZERO).ok();
        
        let per_batch_gas_cost = StorageBackedUint64::new(state, base_key, PER_BATCH_GAS_COST_OFFSET);
        per_batch_gas_cost.set(INITIAL_PER_BATCH_GAS_COST_V6).ok();
        
        let amortized_cost_cap_bips = StorageBackedUint64::new(state, base_key, AMORTIZED_COST_CAP_BIPS_OFFSET);
        amortized_cost_cap_bips.set(0).ok();
        
        let l1_fees_available = StorageBackedBigUint::new(state, base_key, L1_FEES_AVAILABLE_OFFSET);
        l1_fees_available.set(U256::ZERO).ok();
        
        let gas_floor_per_token = StorageBackedUint64::new(state, base_key, GAS_FLOOR_PER_TOKEN_OFFSET);
        gas_floor_per_token.set(0).ok();
    }

    pub fn set_initial_values(
        &self,
        initial_rewards_recipient: Address,
        initial_l1_base_fee: U256,
    ) -> Result<(), ()> {
        self.pay_rewards_to.set(initial_rewards_recipient)?;
        
        let initial_equilibration_units = U256::from(60u64 * 16u64 * 100_000u64);
        self.equilibration_units.set(initial_equilibration_units)?;
        
        self.inertia.set(INITIAL_INERTIA)?;
        self.per_unit_reward.set(INITIAL_PER_UNIT_REWARD)?;
        self.funds_due_for_rewards.set(U256::ZERO)?;
        self.price_per_unit.set(initial_l1_base_fee)?;
        
        self.last_update_time.set(0)?;
        self.units_since_update.set(0)?;
        self.last_surplus.set(U256::ZERO)?;
        self.per_batch_gas_cost.set(INITIAL_PER_BATCH_GAS_COST_V6)?;
        self.amortized_cost_cap_bips.set(0)?;
        self.l1_fees_available.set(U256::ZERO)?;
        self.gas_floor_per_token.set(0)?;
        
        Ok(())
    }

    pub fn get_price_per_unit(&self) -> Result<U256, ()> {
        self.price_per_unit.get()
    }

    pub fn set_price_per_unit(&self, price: U256) -> Result<(), ()> {
        self.price_per_unit.set(price)
    }

    pub fn get_inertia(&self) -> Result<u64, ()> {
        self.inertia.get()
    }

    pub fn set_inertia(&self, inertia: u64) -> Result<(), ()> {
        self.inertia.set(inertia)
    }

    pub fn get_equilibration_units(&self) -> Result<U256, ()> {
        self.equilibration_units.get()
    }

    pub fn set_equilibration_units(&self, units: U256) -> Result<(), ()> {
        self.equilibration_units.set(units)
    }

    pub fn get_per_unit_reward(&self) -> Result<u64, ()> {
        self.per_unit_reward.get()
    }

    pub fn set_per_unit_reward(&self, reward: u64) -> Result<(), ()> {
        self.per_unit_reward.set(reward)
    }

    pub fn get_last_update_time(&self) -> Result<u64, ()> {
        self.last_update_time.get()
    }

    pub fn set_last_update_time(&self, time: u64) -> Result<(), ()> {
        self.last_update_time.set(time)
    }

    pub fn get_units_since_update(&self) -> Result<u64, ()> {
        self.units_since_update.get()
    }

    pub fn add_to_units_since_update(&self, units: u64) -> Result<(), ()> {
        let current = self.units_since_update.get().unwrap_or(0);
        self.units_since_update.set(current.saturating_add(units))
    }

    pub fn get_funds_due_for_rewards(&self) -> Result<U256, ()> {
        self.funds_due_for_rewards.get()
    }

    pub fn set_funds_due_for_rewards(&self, funds: U256) -> Result<(), ()> {
        self.funds_due_for_rewards.set(funds)
    }

    pub fn get_last_surplus(&self) -> Result<U256, ()> {
        self.last_surplus.get()
    }

    pub fn set_last_surplus(&self, surplus: U256) -> Result<(), ()> {
        self.last_surplus.set(surplus)
    }

    pub fn get_per_batch_gas_cost(&self) -> Result<u64, ()> {
        self.per_batch_gas_cost.get()
    }

    pub fn set_per_batch_gas_cost(&self, cost: u64) -> Result<(), ()> {
        self.per_batch_gas_cost.set(cost)
    }

    pub fn get_l1_fees_available(&self) -> Result<U256, ()> {
        self.l1_fees_available.get()
    }

    pub fn add_to_l1_fees_available(&self, amount: U256) -> Result<(), ()> {
        let current = self.l1_fees_available.get().unwrap_or(U256::ZERO);
        self.l1_fees_available.set(current.saturating_add(amount))
    }

    pub fn poster_data_cost(&self, calldata_units: u64) -> Result<U256, ()> {
        let price_per_unit = self.get_price_per_unit()?;
        let per_batch_gas_cost = self.get_per_batch_gas_cost()?;
        
        let calldata_cost = price_per_unit.saturating_mul(U256::from(calldata_units));
        let batch_cost = U256::from(per_batch_gas_cost);
        
        Ok(calldata_cost.saturating_add(batch_cost))
    }

    pub fn update_for_batch_poster_spending(
        &self,
        units_bought: u64,
        calldata_units: u64,
        l1_base_fee: U256,
        current_time: u64,
    ) -> Result<(), ()> {
        let inertia = self.get_inertia()?;
        let equilibration_units = self.get_equilibration_units()?;
        let current_price = self.get_price_per_unit()?;
        
        self.add_to_units_since_update(units_bought)?;
        
        let last_update_time = self.get_last_update_time()?;
        let time_since_update = current_time.saturating_sub(last_update_time);
        
        if time_since_update >= 3600 || self.get_units_since_update()? >= equilibration_units.try_into().unwrap_or(u64::MAX) {
            let target_price = l1_base_fee;
            let price_diff = if target_price > current_price {
                target_price - current_price
            } else {
                current_price - target_price
            };
            
            let adjustment = price_diff / U256::from(inertia);
            let new_price = if target_price > current_price {
                current_price + adjustment
            } else {
                current_price.saturating_sub(adjustment)
            };
            
            self.set_price_per_unit(new_price)?;
            self.set_last_update_time(current_time)?;
            self.units_since_update.set(0)?; // Reset units counter
        }
        
        Ok(())
    }
}

pub fn poster_units_from_brotli_len(brotli_len: u64) -> u64 {
    (brotli_len + 15) / 16
}

pub fn apply_estimation_padding(units: u64) -> u64 {
    units + (units / 10)
}
