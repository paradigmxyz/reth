//! ArbGasInfo REVM precompile implementation.
//!
//! This module implements ArbGasInfo as a proper REVM precompile that intercepts CALL operations
//! to address 0x6c and returns gas pricing information.
//!
//! Key functions:
//! - GetL1BaseFeeEstimate (selector 0xf5d6ded7): Returns L1PricingState.PricePerUnit()
//! - GetL1GasPriceEstimate (selector 0x055f362f): Same as GetL1BaseFeeEstimate
//! - GetMinimumGasPrice (selector 0xf918379a): Returns L2PricingState.MinBaseFeeWei()
//! - GetPricesInWei (selector 0x41b247a8): Returns 6 values for gas pricing
//! - GetGasAccountingParams (selector 0x612af178): Returns speed limit, pool size, tx gas limit
//! - GetCurrentTxL1GasFees (selector 0xc6f7de0e): Returns poster fee for current tx

use alloy_evm::precompiles::{DynPrecompile, PrecompileInput};
use alloy_primitives::{Address, U256, keccak256};
use revm::precompile::{PrecompileId, PrecompileOutput, PrecompileResult, PrecompileError};

/// ArbGasInfo precompile address (0x6c = 108)
pub const ARBGASINFO_ADDRESS: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x6c,
]);

/// ArbOS state address
const ARBOS_STATE_ADDRESS: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x0a, 0x4b, 0x05,
]);

/// Function selectors
/// getL1BaseFeeEstimate() -> 0xf5d6ded7
const GET_L1_BASEFEE_ESTIMATE_SELECTOR: [u8; 4] = [0xf5, 0xd6, 0xde, 0xd7];
/// getL1GasPriceEstimate() -> 0x055f362f
const GET_L1_GAS_PRICE_ESTIMATE_SELECTOR: [u8; 4] = [0x05, 0x5f, 0x36, 0x2f];
/// getMinimumGasPrice() -> 0xf918379a
const GET_MINIMUM_GAS_PRICE_SELECTOR: [u8; 4] = [0xf9, 0x18, 0x37, 0x9a];
/// getPricesInWei() -> 0x41b247a8
const GET_PRICES_IN_WEI_SELECTOR: [u8; 4] = [0x41, 0xb2, 0x47, 0xa8];
/// getGasAccountingParams() -> 0x612af178
const GET_GAS_ACCOUNTING_PARAMS_SELECTOR: [u8; 4] = [0x61, 0x2a, 0xf1, 0x78];
/// getCurrentTxL1GasFees() -> 0xc6f7de0e
const GET_CURRENT_TX_L1_FEES_SELECTOR: [u8; 4] = [0xc6, 0xf7, 0xde, 0x0e];
/// getPricesInArbGas() -> 0x02199f34
const GET_PRICES_IN_ARBGAS_SELECTOR: [u8; 4] = [0x02, 0x19, 0x9f, 0x34];
/// getL1BaseFeeEstimateInertia() -> 0x29eb31ee
const GET_L1_BASEFEE_ESTIMATE_INERTIA_SELECTOR: [u8; 4] = [0x29, 0xeb, 0x31, 0xee];
/// getGasBacklog() -> 0x1d5b5c20
const GET_GAS_BACKLOG_SELECTOR: [u8; 4] = [0x1d, 0x5b, 0x5c, 0x20];
/// getPricingInertia() -> 0x3dfb45b9
const GET_PRICING_INERTIA_SELECTOR: [u8; 4] = [0x3d, 0xfb, 0x45, 0xb9];
/// getGasBacklogTolerance() -> 0x25754f91
const GET_GAS_BACKLOG_TOLERANCE_SELECTOR: [u8; 4] = [0x25, 0x75, 0x4f, 0x91];
/// getL1PricingSurplus() -> 0x520acdd7
const GET_L1_PRICING_SURPLUS_SELECTOR: [u8; 4] = [0x52, 0x0a, 0xcd, 0xd7];
/// getPerBatchGasCharge() -> 0x6ecca45a
const GET_PER_BATCH_GAS_CHARGE_SELECTOR: [u8; 4] = [0x6e, 0xcc, 0xa4, 0x5a];
/// getAmortizedCostCapBips() -> 0x7a7d6beb
const GET_AMORTIZED_COST_CAP_BIPS_SELECTOR: [u8; 4] = [0x7a, 0x7d, 0x6b, 0xeb];
/// getL1FeesAvailable() -> 0x5b39d23c
const GET_L1_FEES_AVAILABLE_SELECTOR: [u8; 4] = [0x5b, 0x39, 0xd2, 0x3c];
/// getL1RewardRate() -> 0x8a5b1d28
const GET_L1_REWARD_RATE_SELECTOR: [u8; 4] = [0x8a, 0x5b, 0x1d, 0x28];
/// getL1RewardRecipient() -> 0x9e6d7de5
const GET_L1_REWARD_RECIPIENT_SELECTOR: [u8; 4] = [0x9e, 0x6d, 0x7d, 0xe5];

/// Gas costs (matching Go nitro)
const SLOAD_GAS_EIP2200: u64 = 800;
const COPY_GAS: u64 = 3;

/// Creates the ArbGasInfo precompile as a DynPrecompile.
/// 
/// This precompile reads L1 and L2 pricing state from storage and returns
/// gas pricing information to callers.
pub fn create_arbgasinfo_precompile() -> DynPrecompile {
    DynPrecompile::new_stateful(PrecompileId::custom("arbgasinfo"), arbgasinfo_precompile_handler)
}

/// Main handler for ArbGasInfo precompile calls.
fn arbgasinfo_precompile_handler(mut input: PrecompileInput<'_>) -> PrecompileResult {
    let data = input.data;
    
    tracing::info!(
        target: "arb::arbgasinfo_precompile",
        data_len = data.len(),
        caller = ?input.caller,
        target = ?input.target_address,
        "ArbGasInfo precompile called"
    );
    
    if data.len() < 4 {
        return Err(PrecompileError::other("Invalid input: too short"));
    }
    
    let selector: [u8; 4] = [data[0], data[1], data[2], data[3]];
    
    tracing::info!(
        target: "arb::arbgasinfo_precompile",
        selector = ?format!("0x{:02x}{:02x}{:02x}{:02x}", selector[0], selector[1], selector[2], selector[3]),
        "ArbGasInfo selector"
    );
    
    match selector {
        GET_L1_BASEFEE_ESTIMATE_SELECTOR => {
            handle_get_l1_basefee_estimate(&mut input)
        }
        GET_L1_GAS_PRICE_ESTIMATE_SELECTOR => {
            // Same as GetL1BaseFeeEstimate
            handle_get_l1_basefee_estimate(&mut input)
        }
        GET_MINIMUM_GAS_PRICE_SELECTOR => {
            handle_get_minimum_gas_price(&mut input)
        }
        GET_PRICES_IN_WEI_SELECTOR => {
            handle_get_prices_in_wei(&mut input)
        }
        GET_GAS_ACCOUNTING_PARAMS_SELECTOR => {
            handle_get_gas_accounting_params(&mut input)
        }
        GET_CURRENT_TX_L1_FEES_SELECTOR => {
            handle_get_current_tx_l1_fees(&mut input)
        }
        GET_PRICES_IN_ARBGAS_SELECTOR => {
            handle_get_prices_in_arbgas(&mut input)
        }
        GET_L1_BASEFEE_ESTIMATE_INERTIA_SELECTOR => {
            handle_get_l1_basefee_estimate_inertia(&mut input)
        }
        GET_GAS_BACKLOG_SELECTOR => {
            handle_get_gas_backlog(&mut input)
        }
        GET_PRICING_INERTIA_SELECTOR => {
            handle_get_pricing_inertia(&mut input)
        }
        GET_GAS_BACKLOG_TOLERANCE_SELECTOR => {
            handle_get_gas_backlog_tolerance(&mut input)
        }
        GET_L1_PRICING_SURPLUS_SELECTOR => {
            handle_get_l1_pricing_surplus(&mut input)
        }
        GET_PER_BATCH_GAS_CHARGE_SELECTOR => {
            handle_get_per_batch_gas_charge(&mut input)
        }
        GET_AMORTIZED_COST_CAP_BIPS_SELECTOR => {
            handle_get_amortized_cost_cap_bips(&mut input)
        }
        GET_L1_FEES_AVAILABLE_SELECTOR => {
            handle_get_l1_fees_available(&mut input)
        }
        GET_L1_REWARD_RATE_SELECTOR => {
            handle_get_l1_reward_rate(&mut input)
        }
        GET_L1_REWARD_RECIPIENT_SELECTOR => {
            handle_get_l1_reward_recipient(&mut input)
        }
        _ => {
            tracing::warn!(
                target: "arb::arbgasinfo_precompile",
                selector = ?hex::encode(selector),
                "Unknown ArbGasInfo selector"
            );
            Err(PrecompileError::other(format!("Unknown selector: 0x{}", hex::encode(selector))))
        }
    }
}

/// Handle getL1BaseFeeEstimate() - returns L1PricingState.PricePerUnit()
fn handle_get_l1_basefee_estimate(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    // Load the ArbOS state account first to ensure it's in the journal
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // Compute storage slot for L1PricingState.pricePerUnit
    // L1 pricing space offset = 7
    // Price per unit offset within L1 pricing = 7
    let l1_pricing_space: u64 = 7;
    let price_per_unit_offset: u64 = 7;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let price_slot = compute_storage_slot(&[l1_space_slot], price_per_unit_offset);
    
    // Read from storage
    let price_per_unit = internals.sload(ARBOS_STATE_ADDRESS, price_slot)
        .map_err(|_| PrecompileError::other("failed to read price per unit"))?;
    
    tracing::debug!(
        target: "arb::arbgasinfo_precompile",
        price_per_unit = ?price_per_unit.data,
        slot = ?price_slot,
        "GetL1BaseFeeEstimate called"
    );
    
    // Return as uint256 (32 bytes)
    let mut output = [0u8; 32];
    output.copy_from_slice(&price_per_unit.data.to_be_bytes::<32>());
    
    // Gas cost: 1 SLOAD + COPY
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getMinimumGasPrice() - returns L2PricingState.MinBaseFeeWei()
fn handle_get_minimum_gas_price(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L2 pricing space offset = 8
    // MinBaseFeeWei offset within L2 pricing = 0
    let l2_pricing_space: u64 = 8;
    let min_basefee_offset: u64 = 0;
    
    let l2_space_slot = compute_storage_slot(&[], l2_pricing_space);
    let min_basefee_slot = compute_storage_slot(&[l2_space_slot], min_basefee_offset);
    
    let min_basefee = internals.sload(ARBOS_STATE_ADDRESS, min_basefee_slot)
        .map_err(|_| PrecompileError::other("failed to read min basefee"))?;
    
    tracing::debug!(
        target: "arb::arbgasinfo_precompile",
        min_basefee = ?min_basefee.data,
        "GetMinimumGasPrice called"
    );
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&min_basefee.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getPricesInWei() - returns 6 values for gas pricing
fn handle_get_prices_in_wei(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // Get L1 price per unit
    let l1_pricing_space: u64 = 7;
    let price_per_unit_offset: u64 = 7;
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let price_slot = compute_storage_slot(&[l1_space_slot], price_per_unit_offset);
    let l1_gas_price = internals.sload(ARBOS_STATE_ADDRESS, price_slot)
        .map_err(|_| PrecompileError::other("failed to read l1 gas price"))?;
    
    // Get L2 basefee from L2 pricing state storage (offset 0 = baseFeeWeiOffset)
    // This matches Go nitro's evm.Context.BaseFee which is set from L2PricingState.BaseFeeWei()
    let l2_pricing_space: u64 = 8;
    let basefee_offset: u64 = 0;  // baseFeeWeiOffset
    let l2_space_slot = compute_storage_slot(&[], l2_pricing_space);
    let basefee_slot = compute_storage_slot(&[l2_space_slot], basefee_offset);
    let l2_gas_price = internals.sload(ARBOS_STATE_ADDRESS, basefee_slot)
        .map_err(|_| PrecompileError::other("failed to read l2 basefee"))?.data;
    
    // Constants from Go nitro
    const TX_DATA_NON_ZERO_GAS_EIP2028: u64 = 16;
    const ASSUMED_SIMPLE_TX_SIZE: u64 = 140;
    const STORAGE_WRITE_COST: u64 = 20000;
    
    // Calculate values matching Go nitro's GetPricesInWeiWithAggregator
    let wei_for_l1_calldata = l1_gas_price.data.saturating_mul(U256::from(TX_DATA_NON_ZERO_GAS_EIP2028));
    let per_l2_tx = wei_for_l1_calldata.saturating_mul(U256::from(ASSUMED_SIMPLE_TX_SIZE));
    
    // Get min base fee for perArbGasBase calculation (offset 1 = minBaseFeeWeiOffset)
    let min_basefee_offset: u64 = 1;  // minBaseFeeWeiOffset
    let min_basefee_slot = compute_storage_slot(&[l2_space_slot], min_basefee_offset);
    let min_basefee = internals.sload(ARBOS_STATE_ADDRESS, min_basefee_slot)
        .map_err(|_| PrecompileError::other("failed to read min basefee"))?;
    
    let per_arbgas_base = if l2_gas_price < min_basefee.data { l2_gas_price } else { min_basefee.data };
    let per_arbgas_congestion = l2_gas_price.saturating_sub(per_arbgas_base);
    let per_arbgas_total = l2_gas_price;
    let wei_for_l2_storage = l2_gas_price.saturating_mul(U256::from(STORAGE_WRITE_COST));
    
    tracing::debug!(
        target: "arb::arbgasinfo_precompile",
        per_l2_tx = ?per_l2_tx,
        wei_for_l1_calldata = ?wei_for_l1_calldata,
        wei_for_l2_storage = ?wei_for_l2_storage,
        per_arbgas_base = ?per_arbgas_base,
        per_arbgas_congestion = ?per_arbgas_congestion,
        per_arbgas_total = ?per_arbgas_total,
        "GetPricesInWei called"
    );
    
    // Return 6 uint256 values (192 bytes)
    let mut output = Vec::with_capacity(192);
    output.extend_from_slice(&per_l2_tx.to_be_bytes::<32>());
    output.extend_from_slice(&wei_for_l1_calldata.to_be_bytes::<32>());
    output.extend_from_slice(&wei_for_l2_storage.to_be_bytes::<32>());
    output.extend_from_slice(&per_arbgas_base.to_be_bytes::<32>());
    output.extend_from_slice(&per_arbgas_congestion.to_be_bytes::<32>());
    output.extend_from_slice(&per_arbgas_total.to_be_bytes::<32>());
    
    // Gas cost: 2 SLOADs + COPY
    let gas_cost = 2 * SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.into()))
}

/// Handle getGasAccountingParams() - returns speed limit, pool size, tx gas limit
fn handle_get_gas_accounting_params(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L2 pricing space offset = 8
    // SpeedLimitPerSecond offset = 1
    // PerBlockGasLimit offset = 2
    let l2_pricing_space: u64 = 8;
    let l2_space_slot = compute_storage_slot(&[], l2_pricing_space);
    
    let speed_limit_slot = compute_storage_slot(&[l2_space_slot], 1);
    let per_block_gas_limit_slot = compute_storage_slot(&[l2_space_slot], 2);
    
    let speed_limit = internals.sload(ARBOS_STATE_ADDRESS, speed_limit_slot)
        .map_err(|_| PrecompileError::other("failed to read speed limit"))?;
    let max_tx_gas_limit = internals.sload(ARBOS_STATE_ADDRESS, per_block_gas_limit_slot)
        .map_err(|_| PrecompileError::other("failed to read max tx gas limit"))?;
    
    tracing::debug!(
        target: "arb::arbgasinfo_precompile",
        speed_limit = ?speed_limit.data,
        max_tx_gas_limit = ?max_tx_gas_limit.data,
        "GetGasAccountingParams called"
    );
    
    // Return 3 uint256 values (96 bytes)
    let mut output = Vec::with_capacity(96);
    output.extend_from_slice(&speed_limit.data.to_be_bytes::<32>());
    output.extend_from_slice(&max_tx_gas_limit.data.to_be_bytes::<32>());
    output.extend_from_slice(&max_tx_gas_limit.data.to_be_bytes::<32>());
    
    let gas_cost = 2 * SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.into()))
}

/// Handle getCurrentTxL1GasFees() - returns poster fee for current tx
fn handle_get_current_tx_l1_fees(_input: &mut PrecompileInput<'_>) -> PrecompileResult {
    // This returns the poster fee for the current transaction
    // For now, return 0 as we don't have access to the tx processor context
    // TODO: Implement proper poster fee tracking
    let output = [0u8; 32];
    let gas_cost = COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getPricesInArbGas() - returns 3 values for gas pricing in ArbGas
fn handle_get_prices_in_arbgas(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // Get L1 price per unit
    let l1_pricing_space: u64 = 7;
    let price_per_unit_offset: u64 = 7;
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let price_slot = compute_storage_slot(&[l1_space_slot], price_per_unit_offset);
    let l1_gas_price = internals.sload(ARBOS_STATE_ADDRESS, price_slot)
        .map_err(|_| PrecompileError::other("failed to read l1 gas price"))?;
    
    // Get L2 basefee from L2 pricing state storage (offset 0 = baseFeeWeiOffset)
    let l2_pricing_space: u64 = 8;
    let basefee_offset: u64 = 0;  // baseFeeWeiOffset
    let l2_space_slot = compute_storage_slot(&[], l2_pricing_space);
    let basefee_slot = compute_storage_slot(&[l2_space_slot], basefee_offset);
    let l2_gas_price = internals.sload(ARBOS_STATE_ADDRESS, basefee_slot)
        .map_err(|_| PrecompileError::other("failed to read l2 basefee"))?.data;
    
    const TX_DATA_NON_ZERO_GAS_EIP2028: u64 = 16;
    const ASSUMED_SIMPLE_TX_SIZE: u64 = 140;
    const STORAGE_WRITE_COST: u64 = 20000;
    
    let wei_for_l1_calldata = l1_gas_price.data.saturating_mul(U256::from(TX_DATA_NON_ZERO_GAS_EIP2028));
    let wei_per_l2_tx = wei_for_l1_calldata.saturating_mul(U256::from(ASSUMED_SIMPLE_TX_SIZE));
    
    let (gas_for_l1_calldata, gas_per_l2_tx) = if l2_gas_price > U256::ZERO {
        (wei_for_l1_calldata / l2_gas_price, wei_per_l2_tx / l2_gas_price)
    } else {
        (U256::ZERO, U256::ZERO)
    };
    
    tracing::debug!(
        target: "arb::arbgasinfo_precompile",
        gas_per_l2_tx = ?gas_per_l2_tx,
        gas_for_l1_calldata = ?gas_for_l1_calldata,
        "GetPricesInArbGas called"
    );
    
    // Return 3 uint256 values (96 bytes)
    let mut output = Vec::with_capacity(96);
    output.extend_from_slice(&gas_per_l2_tx.to_be_bytes::<32>());
    output.extend_from_slice(&gas_for_l1_calldata.to_be_bytes::<32>());
    output.extend_from_slice(&U256::from(STORAGE_WRITE_COST).to_be_bytes::<32>());
    
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.into()))
}

/// Handle getL1BaseFeeEstimateInertia() - returns L1PricingState.Inertia()
fn handle_get_l1_basefee_estimate_inertia(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L1 pricing space offset = 7
    // Inertia offset within L1 pricing = 1
    let l1_pricing_space: u64 = 7;
    let inertia_offset: u64 = 1;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let inertia_slot = compute_storage_slot(&[l1_space_slot], inertia_offset);
    
    let inertia = internals.sload(ARBOS_STATE_ADDRESS, inertia_slot)
        .map_err(|_| PrecompileError::other("failed to read inertia"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&inertia.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getGasBacklog() - returns L2PricingState.GasBacklog()
fn handle_get_gas_backlog(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L2 pricing space offset = 8
    // GasBacklog offset = 3
    let l2_pricing_space: u64 = 8;
    let gas_backlog_offset: u64 = 3;
    
    let l2_space_slot = compute_storage_slot(&[], l2_pricing_space);
    let gas_backlog_slot = compute_storage_slot(&[l2_space_slot], gas_backlog_offset);
    
    let gas_backlog = internals.sload(ARBOS_STATE_ADDRESS, gas_backlog_slot)
        .map_err(|_| PrecompileError::other("failed to read gas backlog"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&gas_backlog.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getPricingInertia() - returns L2PricingState.PricingInertia()
fn handle_get_pricing_inertia(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L2 pricing space offset = 8
    // PricingInertia offset = 4
    let l2_pricing_space: u64 = 8;
    let pricing_inertia_offset: u64 = 4;
    
    let l2_space_slot = compute_storage_slot(&[], l2_pricing_space);
    let pricing_inertia_slot = compute_storage_slot(&[l2_space_slot], pricing_inertia_offset);
    
    let pricing_inertia = internals.sload(ARBOS_STATE_ADDRESS, pricing_inertia_slot)
        .map_err(|_| PrecompileError::other("failed to read pricing inertia"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&pricing_inertia.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getGasBacklogTolerance() - returns L2PricingState.BacklogTolerance()
fn handle_get_gas_backlog_tolerance(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L2 pricing space offset = 8
    // BacklogTolerance offset = 5
    let l2_pricing_space: u64 = 8;
    let backlog_tolerance_offset: u64 = 5;
    
    let l2_space_slot = compute_storage_slot(&[], l2_pricing_space);
    let backlog_tolerance_slot = compute_storage_slot(&[l2_space_slot], backlog_tolerance_offset);
    
    let backlog_tolerance = internals.sload(ARBOS_STATE_ADDRESS, backlog_tolerance_slot)
        .map_err(|_| PrecompileError::other("failed to read backlog tolerance"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&backlog_tolerance.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getL1PricingSurplus() - returns L1PricingState.GetL1PricingSurplus()
fn handle_get_l1_pricing_surplus(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L1 pricing space offset = 7
    // LastSurplus offset = 12 (for ArbOS version >= 10)
    let l1_pricing_space: u64 = 7;
    let last_surplus_offset: u64 = 12;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let last_surplus_slot = compute_storage_slot(&[l1_space_slot], last_surplus_offset);
    
    let last_surplus = internals.sload(ARBOS_STATE_ADDRESS, last_surplus_slot)
        .map_err(|_| PrecompileError::other("failed to read last surplus"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&last_surplus.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getPerBatchGasCharge() - returns L1PricingState.PerBatchGasCost()
fn handle_get_per_batch_gas_charge(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L1 pricing space offset = 7
    // PerBatchGasCost offset = 2
    let l1_pricing_space: u64 = 7;
    let per_batch_gas_cost_offset: u64 = 2;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let per_batch_gas_cost_slot = compute_storage_slot(&[l1_space_slot], per_batch_gas_cost_offset);
    
    let per_batch_gas_cost = internals.sload(ARBOS_STATE_ADDRESS, per_batch_gas_cost_slot)
        .map_err(|_| PrecompileError::other("failed to read per batch gas cost"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&per_batch_gas_cost.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getAmortizedCostCapBips() - returns L1PricingState.AmortizedCostCapBips()
fn handle_get_amortized_cost_cap_bips(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L1 pricing space offset = 7
    // AmortizedCostCapBips offset = 3
    let l1_pricing_space: u64 = 7;
    let amortized_cost_cap_bips_offset: u64 = 3;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let amortized_cost_cap_bips_slot = compute_storage_slot(&[l1_space_slot], amortized_cost_cap_bips_offset);
    
    let amortized_cost_cap_bips = internals.sload(ARBOS_STATE_ADDRESS, amortized_cost_cap_bips_slot)
        .map_err(|_| PrecompileError::other("failed to read amortized cost cap bips"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&amortized_cost_cap_bips.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getL1FeesAvailable() - returns L1PricingState.L1FeesAvailable()
fn handle_get_l1_fees_available(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L1 pricing space offset = 7
    // L1FeesAvailable offset = 8
    let l1_pricing_space: u64 = 7;
    let l1_fees_available_offset: u64 = 8;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let l1_fees_available_slot = compute_storage_slot(&[l1_space_slot], l1_fees_available_offset);
    
    let l1_fees_available = internals.sload(ARBOS_STATE_ADDRESS, l1_fees_available_slot)
        .map_err(|_| PrecompileError::other("failed to read l1 fees available"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&l1_fees_available.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getL1RewardRate() - returns L1PricingState.PerUnitReward()
fn handle_get_l1_reward_rate(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L1 pricing space offset = 7
    // PerUnitReward offset = 4
    let l1_pricing_space: u64 = 7;
    let per_unit_reward_offset: u64 = 4;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let per_unit_reward_slot = compute_storage_slot(&[l1_space_slot], per_unit_reward_offset);
    
    let per_unit_reward = internals.sload(ARBOS_STATE_ADDRESS, per_unit_reward_slot)
        .map_err(|_| PrecompileError::other("failed to read per unit reward"))?;
    
    let mut output = [0u8; 32];
    output.copy_from_slice(&per_unit_reward.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Handle getL1RewardRecipient() - returns L1PricingState.PayRewardsTo()
fn handle_get_l1_reward_recipient(input: &mut PrecompileInput<'_>) -> PrecompileResult {
    let internals = input.internals_mut();
    
    let _ = internals.load_account(ARBOS_STATE_ADDRESS)
        .map_err(|e| PrecompileError::other(format!("load_account failed: {:?}", e)))?;
    
    // L1 pricing space offset = 7
    // PayRewardsTo offset = 5
    let l1_pricing_space: u64 = 7;
    let pay_rewards_to_offset: u64 = 5;
    
    let l1_space_slot = compute_storage_slot(&[], l1_pricing_space);
    let pay_rewards_to_slot = compute_storage_slot(&[l1_space_slot], pay_rewards_to_offset);
    
    let pay_rewards_to = internals.sload(ARBOS_STATE_ADDRESS, pay_rewards_to_slot)
        .map_err(|_| PrecompileError::other("failed to read pay rewards to"))?;
    
    // Return as address (32 bytes, left-padded)
    let mut output = [0u8; 32];
    output.copy_from_slice(&pay_rewards_to.data.to_be_bytes::<32>());
    let gas_cost = SLOAD_GAS_EIP2200 + COPY_GAS;
    
    Ok(PrecompileOutput::new(gas_cost, output.to_vec().into()))
}

/// Compute storage slot using the same method as ArbSys precompile
fn compute_storage_slot(parents: &[U256], offset: u64) -> U256 {
    if parents.is_empty() {
        return U256::from(offset);
    }
    
    // For nested storage, hash the parent slot with "Arbitrum internal storage" prefix
    let parent = parents[0];
    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(b"Arbitrum internal storage");
    data.extend_from_slice(&parent.to_be_bytes::<32>());
    
    let base = U256::from_be_bytes(keccak256(&data).0);
    base.wrapping_add(U256::from(offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_selectors() {
        // Verify selectors match expected values
        assert_eq!(GET_L1_BASEFEE_ESTIMATE_SELECTOR, [0xf5, 0xd6, 0xde, 0xd7]);
        assert_eq!(GET_L1_GAS_PRICE_ESTIMATE_SELECTOR, [0x05, 0x5f, 0x36, 0x2f]);
        assert_eq!(GET_MINIMUM_GAS_PRICE_SELECTOR, [0xf9, 0x18, 0x37, 0x9a]);
        assert_eq!(GET_PRICES_IN_WEI_SELECTOR, [0x41, 0xb2, 0x47, 0xa8]);
        assert_eq!(GET_GAS_ACCOUNTING_PARAMS_SELECTOR, [0x61, 0x2a, 0xf1, 0x78]);
        assert_eq!(GET_CURRENT_TX_L1_FEES_SELECTOR, [0xc6, 0xf7, 0xde, 0x0e]);
    }
}
