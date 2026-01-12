use alloy_primitives::B256;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    // Stores (gas_used, cumulative_gas, success_flag) for early-terminated transactions
    static EARLY_TX_GAS: RefCell<HashMap<B256, (u64, u64, bool)>> = RefCell::new(HashMap::new());
    static BLOCK_GAS_ADJUSTMENT: RefCell<i64> = RefCell::new(0);
}

pub fn set_early_tx_gas(tx_hash: B256, gas_used: u64, cumulative_gas: u64) {
    // Default success=true for backward compatibility
    set_early_tx_gas_with_status(tx_hash, gas_used, cumulative_gas, true);
}

pub fn set_early_tx_gas_with_status(tx_hash: B256, gas_used: u64, cumulative_gas: u64, success: bool) {
    tracing::debug!(
        target: "arb-reth::gas-tracking",
        tx_hash = ?tx_hash,
        gas_used = gas_used,
        cumulative_gas = cumulative_gas,
        success = success,
        "STORING early_tx_gas with status"
    );
    EARLY_TX_GAS.with(|map| {
        map.borrow_mut().insert(tx_hash, (gas_used, cumulative_gas, success));
    });
}

/// Returns (gas_used, cumulative_gas, success) for early-terminated transactions
pub fn get_early_tx_gas(tx_hash: &B256) -> Option<(u64, u64, bool)> {
    let result = EARLY_TX_GAS.with(|map| {
        map.borrow().get(tx_hash).copied()
    });
    tracing::debug!(
        target: "arb-reth::gas-tracking",
        tx_hash = ?tx_hash,
        found = result.is_some(),
        result = ?result,
        "RETRIEVING early_tx_gas"
    );
    result
}

pub fn clear_early_tx_gas(tx_hash: &B256) {
    tracing::debug!(
        target: "arb-reth::gas-tracking",
        tx_hash = ?tx_hash,
        "CLEARING early_tx_gas"
    );
    EARLY_TX_GAS.with(|map| {
        map.borrow_mut().remove(tx_hash);
    });
}

pub fn add_gas_adjustment(adjustment: i64) {
    BLOCK_GAS_ADJUSTMENT.with(|adj| {
        *adj.borrow_mut() += adjustment;
    });
}

pub fn get_and_clear_gas_adjustment() -> i64 {
    BLOCK_GAS_ADJUSTMENT.with(|adj| {
        let value = *adj.borrow();
        *adj.borrow_mut() = 0;
        value
    })
}
