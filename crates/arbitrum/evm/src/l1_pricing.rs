// L1 Pricing State implementation matching Go nitro's arbos/l1pricing/l1pricing.go
//
// This module handles L1 pricing for Arbitrum, including:
// - Tracking batch poster spending
// - Computing L1 data costs
// - Managing rewards and payments to batch posters

use alloy_primitives::{Address, B256, U256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedBigUint, StorageBackedAddress, StorageBackedInt64, StorageBackedBigInt};
use crate::batch_poster::BatchPostersTable;

/// Performs signed floor division matching Go's big.Int.Div semantics.
/// Go's Div uses Euclidean division which rounds towards negative infinity.
/// 
/// For positive dividend: floor(a/b) = a/b (truncation)
/// For negative dividend with positive divisor: if there's a remainder, subtract 1 from truncated result
/// 
/// Returns (quotient_magnitude, quotient_is_positive)
fn signed_floor_div(
    dividend_magnitude: U256,
    dividend_positive: bool,
    divisor: U256,
) -> (U256, bool) {
    if divisor.is_zero() {
        return (U256::ZERO, true);
    }
    
    let quotient = dividend_magnitude.checked_div(divisor).unwrap_or(U256::ZERO);
    let remainder = dividend_magnitude.checked_rem(divisor).unwrap_or(U256::ZERO);
    
    if dividend_positive {
        // Positive dividend: truncation = floor division
        (quotient, true)
    } else {
        // Negative dividend: floor division rounds towards negative infinity
        // If there's a remainder, we need to add 1 to the magnitude (making it more negative)
        if remainder.is_zero() {
            (quotient, false)
        } else {
            (quotient.saturating_add(U256::from(1)), false)
        }
    }
}

fn compress_brotli(data: &[u8], level: u64) -> Result<u64, ()> {
    use brotli::enc::BrotliEncoderParams;
    
    let quality = level.min(11) as u32;
    
    let mut params = BrotliEncoderParams::default();
    params.quality = quality as i32;
    
    let mut compressed = Vec::new();
    let mut cursor = std::io::Cursor::new(data);
    
    match brotli::BrotliCompress(&mut cursor, &mut compressed, &params) {
        Ok(_) => Ok(compressed.len() as u64),
        Err(_) => Err(()),
    }
}

pub struct L1PricingState<D> {
    pub storage: Storage<D>,
    
    pay_rewards_to: StorageBackedAddress<D>,
    equilibration_units: StorageBackedBigUint<D>,
    inertia: StorageBackedUint64<D>,
    per_unit_reward: StorageBackedUint64<D>,
    
    last_update_time: StorageBackedUint64<D>,
    funds_due_for_rewards: StorageBackedBigInt<D>,  // SIGNED - can be negative
    units_since_update: StorageBackedUint64<D>,
    price_per_unit: StorageBackedBigUint<D>,
    last_surplus: StorageBackedBigInt<D>,  // SIGNED - can be negative
    per_batch_gas_cost: StorageBackedInt64<D>,  // SIGNED - can be negative
    amortized_cost_cap_bips: StorageBackedUint64<D>,
    l1_fees_available: StorageBackedBigUint<D>,
    gas_floor_per_token: StorageBackedUint64<D>,
    
    pub arbos_version: u64,
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
const INITIAL_PER_BATCH_GAS_COST_V6: i64 = 100_000;
const INITIAL_PER_BATCH_GAS_COST_V12: i64 = 210_000;

// 0xa4b000000000000000000073657175656e636572
// = 0xa4b0 + 11 zero bytes + "sequencer" (9 bytes = 0x73657175656e636572)
pub const BATCH_POSTER_ADDRESS: Address = Address::new([
    0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x73, 0x65, 0x71, 0x75, 0x65,
    0x6e, 0x63, 0x65, 0x72
]);

pub const L1_PRICER_FUNDS_POOL_ADDRESS: Address = Address::new([
    0xa4, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0xf6
]);

// ArbOS version constants from Go nitro params
const ARBOS_VERSION_3: u64 = 3;
const ARBOS_VERSION_10: u64 = 10;

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
            funds_due_for_rewards: StorageBackedBigInt::new(state, base_key, FUNDS_DUE_FOR_REWARDS_OFFSET),
            units_since_update: StorageBackedUint64::new(state, base_key, UNITS_SINCE_OFFSET),
            price_per_unit: StorageBackedBigUint::new(state, base_key, PRICE_PER_UNIT_OFFSET),
            last_surplus: StorageBackedBigInt::new(state, base_key, LAST_SURPLUS_OFFSET),
            per_batch_gas_cost: StorageBackedInt64::new(state, base_key, PER_BATCH_GAS_COST_OFFSET),
            amortized_cost_cap_bips: StorageBackedUint64::new(state, base_key, AMORTIZED_COST_CAP_BIPS_OFFSET),
            l1_fees_available: StorageBackedBigUint::new(state, base_key, L1_FEES_AVAILABLE_OFFSET),
            gas_floor_per_token: StorageBackedUint64::new(state, base_key, GAS_FLOOR_PER_TOKEN_OFFSET),
            storage,
            arbos_version,
        }
    }

    pub fn batch_poster_table(&self) -> BatchPostersTable<D> {
        BatchPostersTable::open(&self.storage)
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
        
        let funds_due_for_rewards = StorageBackedBigInt::new(state, base_key, FUNDS_DUE_FOR_REWARDS_OFFSET);
        funds_due_for_rewards.set(U256::ZERO).ok();
        
        let price_per_unit = StorageBackedBigUint::new(state, base_key, PRICE_PER_UNIT_OFFSET);
        price_per_unit.set(initial_l1_base_fee).ok();
        
        let last_update_time = StorageBackedUint64::new(state, base_key, LAST_UPDATE_TIME_OFFSET);
        last_update_time.set(0).ok();
        
        let units_since_update = StorageBackedUint64::new(state, base_key, UNITS_SINCE_OFFSET);
        units_since_update.set(0).ok();
        
        let last_surplus = StorageBackedBigInt::new(state, base_key, LAST_SURPLUS_OFFSET);
        last_surplus.set(U256::ZERO).ok();
        
        let per_batch_gas_cost = StorageBackedInt64::new(state, base_key, PER_BATCH_GAS_COST_OFFSET);
        per_batch_gas_cost.set(INITIAL_PER_BATCH_GAS_COST_V6 as i64).ok();
        
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

    pub fn get_pay_rewards_to(&self) -> Result<Address, ()> {
        self.pay_rewards_to.get()
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
        self.funds_due_for_rewards.get_raw()
    }

    pub fn set_funds_due_for_rewards(&self, funds: U256) -> Result<(), ()> {
        self.funds_due_for_rewards.set(funds)
    }

    pub fn get_last_surplus(&self) -> Result<U256, ()> {
        self.last_surplus.get_raw()
    }

    pub fn set_last_surplus(&self, surplus: U256) -> Result<(), ()> {
        self.last_surplus.set(surplus)
    }

    pub fn get_per_batch_gas_cost(&self) -> Result<i64, ()> {
        self.per_batch_gas_cost.get()
    }

    pub fn set_per_batch_gas_cost(&self, cost: i64) -> Result<(), ()> {
        self.per_batch_gas_cost.set(cost)
    }

    pub fn get_amortized_cost_cap_bips(&self) -> Result<u64, ()> {
        self.amortized_cost_cap_bips.get()
    }

    pub fn set_units_since_update(&self, units: u64) -> Result<(), ()> {
        self.units_since_update.set(units)
    }

    pub fn set_l1_fees_available(&self, val: U256) -> Result<(), ()> {
        self.l1_fees_available.set(val)
    }

    pub fn parent_gas_floor_per_token(&self) -> Result<u64, ()> {
        if self.arbos_version < 50 {
            return Ok(0);
        }
        self.gas_floor_per_token.get()
    }

    pub fn set_parent_gas_floor_per_token(&self, floor: u64) -> Result<(), ()> {
        if self.arbos_version < 50 {
            return Err(());
        }
        self.gas_floor_per_token.set(floor)
    }

    pub fn get_l1_fees_available(&self) -> Result<U256, ()> {
        self.l1_fees_available.get()
    }

    pub fn add_to_l1_fees_available(&self, amount: U256) -> Result<(), ()> {
        let current = self.l1_fees_available.get().unwrap_or(U256::ZERO);
        self.l1_fees_available.set(current.saturating_add(amount))
    }

    pub fn get_poster_data_cost(&self, tx_data: &[u8], poster: Address, brotli_level: u64) -> Result<(U256, u64), ()> {
        if poster != BATCH_POSTER_ADDRESS {
            return Ok((U256::ZERO, 0));
        }
        
        let compressed_len = match compress_brotli(tx_data, brotli_level) {
            Ok(len) => len,
            Err(_) => {
                tracing::error!("Failed to compress tx data with Brotli");
                return Err(());
            }
        };
        
        const TX_DATA_NONZERO_GAS: u64 = 16;
        let units = compressed_len.saturating_mul(TX_DATA_NONZERO_GAS);
        
        let price_per_unit = self.get_price_per_unit()?;
        
        let cost = price_per_unit.saturating_mul(U256::from(units));
        
        Ok((cost, units))
    }
    
    pub fn poster_data_cost(&self, calldata_units: u64) -> Result<U256, ()> {
        let price_per_unit = self.get_price_per_unit()?;
        let per_batch_gas_cost = self.get_per_batch_gas_cost()?;
        
        let calldata_cost = price_per_unit.saturating_mul(U256::from(calldata_units));
        // per_batch_gas_cost can be negative, but for cost calculation we use absolute value
        let batch_cost = U256::from(per_batch_gas_cost.unsigned_abs());
        
        Ok(calldata_cost.saturating_add(batch_cost))
    }

    /// UpdateForBatchPosterSpending updates the pricing model based on a payment by a batch poster.
    /// This matches Go nitro's UpdateForBatchPosterSpending in arbos/l1pricing/l1pricing.go
    ///
    /// Parameters:
    /// - update_time: The timestamp of the batch (from the batch posting report)
    /// - current_time: The current block timestamp
    /// - batch_poster: The address of the batch poster
    /// - wei_spent: The amount of wei spent by the batch poster
    /// - l1_basefee: The L1 base fee at the time of the batch
    /// - transfer_balance_fn: A function to transfer balance between accounts
    pub fn update_for_batch_poster_spending<F>(
        &self,
        update_time: u64,
        current_time: u64,
        batch_poster: Address,
        wei_spent: U256,
        l1_basefee: U256,
        mut transfer_balance_fn: F,
    ) -> Result<(), String>
    where
        F: FnMut(Address, Address, U256) -> Result<(), String>,
    {
        // For ArbOS version < 10, use the old algorithm
        if self.arbos_version < ARBOS_VERSION_10 {
            tracing::warn!("UpdateForBatchPosterSpending: ArbOS version {} < 10, using simplified algorithm", self.arbos_version);
            return self.update_for_batch_poster_spending_simple(update_time, current_time, wei_spent, l1_basefee);
        }

        let batch_poster_table = self.batch_poster_table();
        
        // Open the poster state, creating if it doesn't exist
        let poster_state = match batch_poster_table.open_poster(batch_poster, true) {
            Ok(state) => state,
            Err(_) => {
                return Err(format!("Failed to open poster state for {}", batch_poster));
            }
        };

        let funds_due_for_rewards = self.get_funds_due_for_rewards().unwrap_or(U256::ZERO);
        let l1_fees_available = self.get_l1_fees_available().unwrap_or(U256::ZERO);

        // Compute allocation fraction
        let mut last_update_time = self.get_last_update_time().unwrap_or(0);
        if last_update_time == 0 && update_time > 0 {
            // First update, so there isn't a last update time
            last_update_time = update_time.saturating_sub(1);
        }
        
        if update_time > current_time || update_time < last_update_time {
            return Err(format!("Invalid time: update_time={}, current_time={}, last_update_time={}", 
                update_time, current_time, last_update_time));
        }

        let allocation_numerator = update_time.saturating_sub(last_update_time);
        let allocation_denominator = current_time.saturating_sub(last_update_time);
        let (allocation_numerator, allocation_denominator) = if allocation_denominator == 0 {
            (1u64, 1u64)
        } else {
            (allocation_numerator, allocation_denominator)
        };

        // Allocate units to this update
        let units_since_update = self.get_units_since_update().unwrap_or(0);
        let units_allocated = units_since_update
            .saturating_mul(allocation_numerator)
            .checked_div(allocation_denominator)
            .unwrap_or(0);
        let units_since_update = units_since_update.saturating_sub(units_allocated);
        self.set_units_since_update(units_since_update).ok();

        // Impose cap on amortized cost, if there is one (ArbOS version >= 3)
        let mut wei_spent = wei_spent;
        if self.arbos_version >= ARBOS_VERSION_3 {
            let amortized_cost_cap_bips = self.get_amortized_cost_cap_bips().unwrap_or(0);
            if amortized_cost_cap_bips != 0 {
                // wei_spent_cap = l1_basefee * units_allocated * amortized_cost_cap_bips / 10000
                let wei_spent_cap = l1_basefee
                    .saturating_mul(U256::from(units_allocated))
                    .saturating_mul(U256::from(amortized_cost_cap_bips))
                    .checked_div(U256::from(10000u64))
                    .unwrap_or(U256::MAX);
                if wei_spent_cap < wei_spent {
                    wei_spent = wei_spent_cap;
                }
            }
        }

        // Update funds due to poster
        let due_to_poster = poster_state.get_funds_due().unwrap_or(U256::ZERO);
        let new_due_to_poster = due_to_poster.saturating_add(wei_spent);
        poster_state.set_funds_due(new_due_to_poster, &batch_poster_table.total_funds_due).ok();

        // Update funds due for rewards
        let per_unit_reward = self.get_per_unit_reward().unwrap_or(0);
        let reward_amount = U256::from(units_allocated).saturating_mul(U256::from(per_unit_reward));
        let new_funds_due_for_rewards = funds_due_for_rewards.saturating_add(reward_amount);
        self.set_funds_due_for_rewards(new_funds_due_for_rewards).ok();

        // Pay rewards, as much as possible
        let mut payment_for_rewards = U256::from(per_unit_reward).saturating_mul(U256::from(units_allocated));
        let mut l1_fees_available = l1_fees_available;
        if l1_fees_available < payment_for_rewards {
            payment_for_rewards = l1_fees_available;
        }
        let funds_due_for_rewards = new_funds_due_for_rewards.saturating_sub(payment_for_rewards);
        self.set_funds_due_for_rewards(funds_due_for_rewards).ok();

        let pay_rewards_to = self.get_pay_rewards_to().unwrap_or(Address::ZERO);
        if payment_for_rewards > U256::ZERO {
            // Transfer from L1PricerFundsPoolAddress to payRewardsTo
            transfer_balance_fn(L1_PRICER_FUNDS_POOL_ADDRESS, pay_rewards_to, payment_for_rewards)?;
            l1_fees_available = l1_fees_available.saturating_sub(payment_for_rewards);
            self.set_l1_fees_available(l1_fees_available).ok();
        }

        // Settle up payments owed to the batch poster, as much as possible
        let balance_due_to_poster = poster_state.get_funds_due().unwrap_or(U256::ZERO);
        let mut balance_to_transfer = balance_due_to_poster;
        if l1_fees_available < balance_to_transfer {
            balance_to_transfer = l1_fees_available;
        }
        if balance_to_transfer > U256::ZERO {
            let addr_to_pay = poster_state.get_pay_to().unwrap_or(batch_poster);
            // Transfer from L1PricerFundsPoolAddress to poster's payTo address
            transfer_balance_fn(L1_PRICER_FUNDS_POOL_ADDRESS, addr_to_pay, balance_to_transfer)?;
            l1_fees_available = l1_fees_available.saturating_sub(balance_to_transfer);
            self.set_l1_fees_available(l1_fees_available).ok();
            
            let new_balance_due = balance_due_to_poster.saturating_sub(balance_to_transfer);
            poster_state.set_funds_due(new_balance_due, &batch_poster_table.total_funds_due).ok();
        }

        // Update time
        self.set_last_update_time(update_time).ok();

        // Adjust the price - matching Go nitro's exact algorithm
        if units_allocated > 0 {
            let total_funds_due = batch_poster_table.total_funds_due().unwrap_or(U256::ZERO);
            let funds_due_for_rewards = self.get_funds_due_for_rewards().unwrap_or(U256::ZERO);
            
            // surplus = l1_fees_available - (total_funds_due + funds_due_for_rewards)
            // This can be negative, so we need to handle signed arithmetic
            let need_funds = total_funds_due.saturating_add(funds_due_for_rewards);
            let surplus_positive = l1_fees_available >= need_funds;
            let surplus_magnitude = if surplus_positive {
                l1_fees_available.saturating_sub(need_funds)
            } else {
                need_funds.saturating_sub(l1_fees_available)
            };

            let inertia = self.get_inertia().unwrap_or(INITIAL_INERTIA);
            let equil_units = self.get_equilibration_units().unwrap_or(U256::from(60u64 * 16u64 * 100_000u64));
            let inertia_units = equil_units.checked_div(U256::from(inertia)).unwrap_or(U256::ZERO);
            let price = self.get_price_per_unit().unwrap_or(U256::ZERO);

            let alloc_plus_inert = inertia_units.saturating_add(U256::from(units_allocated));

            // Get old surplus for derivative calculation
            let old_surplus_raw = self.get_last_surplus().unwrap_or(U256::ZERO);
            let old_surplus_negative = self.last_surplus.is_negative().unwrap_or(false);
            
            // Go nitro price adjustment formula:
            // desiredDerivative := -surplus / equilUnits
            // actualDerivative := (surplus - oldSurplus) / unitsAllocated
            // changeDerivativeBy := desiredDerivative - actualDerivative
            // priceChange := changeDerivativeBy * unitsAllocated / allocPlusInert
            //
            // Simplified: priceChange = (-surplus/equilUnits - (surplus-oldSurplus)/unitsAllocated) * unitsAllocated / allocPlusInert
            //
            // For the first batch (oldSurplus = 0, surplus is negative since we have no funds but owe funds):
            // desiredDerivative = -(-magnitude) / equilUnits = magnitude / equilUnits (positive, want to increase price)
            // actualDerivative = (-magnitude - 0) / unitsAllocated = -magnitude / unitsAllocated (negative)
            // changeDerivativeBy = magnitude/equilUnits - (-magnitude/unitsAllocated) = magnitude/equilUnits + magnitude/unitsAllocated
            // priceChange = changeDerivativeBy * unitsAllocated / allocPlusInert
            
            // For simplicity, we'll compute this using signed arithmetic approximation
            // Since U256 doesn't support signed operations, we track sign separately
            
            let units_allocated_u256 = U256::from(units_allocated);
            
            // Compute desiredDerivative = -surplus / equilUnits using floor division
            // Go's big.Int.Div uses Euclidean/floor division (rounds towards negative infinity)
            // If surplus is positive, -surplus is negative, so desiredDerivative is negative
            // If surplus is negative, -surplus is positive, so desiredDerivative is positive
            let (desired_deriv_magnitude, desired_deriv_positive) = signed_floor_div(
                surplus_magnitude,
                !surplus_positive,  // -surplus has opposite sign of surplus
                equil_units,
            );
            
            // Compute actualDerivative = (surplus - oldSurplus) / unitsAllocated
            // Need to compute surplus - oldSurplus with proper sign handling
            let (surplus_diff_magnitude, surplus_diff_positive) = if surplus_positive && !old_surplus_negative {
                // Both positive: surplus - oldSurplus
                if surplus_magnitude >= old_surplus_raw {
                    (surplus_magnitude.saturating_sub(old_surplus_raw), true)
                } else {
                    (old_surplus_raw.saturating_sub(surplus_magnitude), false)
                }
            } else if !surplus_positive && old_surplus_negative {
                // Both negative: -surplus_mag - (-old_surplus_mag) = old_surplus_mag - surplus_mag
                if old_surplus_raw >= surplus_magnitude {
                    (old_surplus_raw.saturating_sub(surplus_magnitude), true)
                } else {
                    (surplus_magnitude.saturating_sub(old_surplus_raw), false)
                }
            } else if surplus_positive && old_surplus_negative {
                // surplus positive, oldSurplus negative: surplus - (-old) = surplus + old
                (surplus_magnitude.saturating_add(old_surplus_raw), true)
            } else {
                // surplus negative, oldSurplus positive: -surplus - old = -(surplus + old)
                (surplus_magnitude.saturating_add(old_surplus_raw), false)
            };
            
            // Compute actualDerivative = (surplus - oldSurplus) / unitsAllocated using floor division
            let (actual_deriv_magnitude, actual_deriv_positive) = signed_floor_div(
                surplus_diff_magnitude,
                surplus_diff_positive,
                units_allocated_u256,
            );
            
            // Compute changeDerivativeBy = desiredDerivative - actualDerivative
            let (change_deriv_magnitude, change_deriv_positive) = if desired_deriv_positive && actual_deriv_positive {
                // Both positive
                if desired_deriv_magnitude >= actual_deriv_magnitude {
                    (desired_deriv_magnitude.saturating_sub(actual_deriv_magnitude), true)
                } else {
                    (actual_deriv_magnitude.saturating_sub(desired_deriv_magnitude), false)
                }
            } else if !desired_deriv_positive && !actual_deriv_positive {
                // Both negative: -d - (-a) = a - d
                if actual_deriv_magnitude >= desired_deriv_magnitude {
                    (actual_deriv_magnitude.saturating_sub(desired_deriv_magnitude), true)
                } else {
                    (desired_deriv_magnitude.saturating_sub(actual_deriv_magnitude), false)
                }
            } else if desired_deriv_positive && !actual_deriv_positive {
                // desired positive, actual negative: d - (-a) = d + a
                (desired_deriv_magnitude.saturating_add(actual_deriv_magnitude), true)
            } else {
                // desired negative, actual positive: -d - a = -(d + a)
                (desired_deriv_magnitude.saturating_add(actual_deriv_magnitude), false)
            };
            
            // Compute priceChange = changeDerivativeBy * unitsAllocated / allocPlusInert using floor division
            // First multiply, then divide with floor semantics
            let change_times_units = change_deriv_magnitude.saturating_mul(units_allocated_u256);
            let (price_change, price_change_positive) = signed_floor_div(
                change_times_units,
                change_deriv_positive,
                alloc_plus_inert,
            );

            // newPrice = price + priceChange (with sign from floor division)
            let new_price = if price_change_positive {
                price.saturating_add(price_change)
            } else {
                price.saturating_sub(price_change)
            };

            // Store surplus for next iteration
            if surplus_positive {
                self.set_last_surplus(surplus_magnitude).ok();
            } else {
                // Store negative surplus using two's complement
                self.last_surplus.set_negative(surplus_magnitude).ok();
            }

            self.set_price_per_unit(new_price).ok();
        }

        Ok(())
    }

    /// Simplified version of update_for_batch_poster_spending for older ArbOS versions
    fn update_for_batch_poster_spending_simple(
        &self,
        update_time: u64,
        current_time: u64,
        wei_spent: U256,
        l1_basefee: U256,
    ) -> Result<(), String> {
        let inertia = self.get_inertia().unwrap_or(INITIAL_INERTIA);
        let equilibration_units = self.get_equilibration_units().unwrap_or(U256::from(60u64 * 16u64 * 100_000u64));
        let current_price = self.get_price_per_unit().unwrap_or(U256::ZERO);
        
        // Calculate units from wei_spent and l1_basefee
        let units_bought = if l1_basefee > U256::ZERO {
            wei_spent.checked_div(l1_basefee).unwrap_or(U256::ZERO).try_into().unwrap_or(u64::MAX)
        } else {
            0u64
        };
        
        self.add_to_units_since_update(units_bought).ok();
        
        let last_update_time = self.get_last_update_time().unwrap_or(0);
        let time_since_update = current_time.saturating_sub(last_update_time);
        
        if time_since_update >= 3600 || self.get_units_since_update().unwrap_or(0) >= equilibration_units.try_into().unwrap_or(u64::MAX) {
            let target_price = l1_basefee;
            let price_diff = if target_price > current_price {
                target_price - current_price
            } else {
                current_price - target_price
            };
            
            let adjustment = price_diff.checked_div(U256::from(inertia)).unwrap_or(U256::ZERO);
            let new_price = if target_price > current_price {
                current_price + adjustment
            } else {
                current_price.saturating_sub(adjustment)
            };
            
            self.set_price_per_unit(new_price).ok();
            self.set_last_update_time(current_time).ok();
            self.units_since_update.set(0).ok();
        }
        
        Ok(())
    }

    /// Legacy update function for backward compatibility (old signature)
    pub fn update_for_batch_poster_spending_legacy(
        &self,
        units_bought: u64,
        _calldata_units: u64,
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
            self.units_since_update.set(0)?;
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
