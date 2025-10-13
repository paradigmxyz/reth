use alloy_primitives::B256;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static EARLY_TX_GAS: RefCell<HashMap<B256, (u64, u64)>> = RefCell::new(HashMap::new());
    static BLOCK_GAS_ADJUSTMENT: RefCell<i64> = RefCell::new(0);
}

pub fn set_early_tx_gas(tx_hash: B256, gas_used: u64, cumulative_gas: u64) {
    tracing::info!(
        target: "arb-reth::gas-tracking",
        tx_hash = ?tx_hash,
        gas_used = gas_used,
        cumulative_gas = cumulative_gas,
        "STORING early_tx_gas"
    );
    EARLY_TX_GAS.with(|map| {
        map.borrow_mut().insert(tx_hash, (gas_used, cumulative_gas));
    });
}

pub fn get_early_tx_gas(tx_hash: &B256) -> Option<(u64, u64)> {
    let result = EARLY_TX_GAS.with(|map| {
        map.borrow().get(tx_hash).copied()
    });
    tracing::info!(
        target: "arb-reth::gas-tracking",
        tx_hash = ?tx_hash,
        found = result.is_some(),
        result = ?result,
        "RETRIEVING early_tx_gas"
    );
    result
}

pub fn clear_early_tx_gas(tx_hash: &B256) {
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
