//! Online feasibility check for the BAL execution path (check D in `BAL.md` §Validation chain).
//!
//! After each tx commits, we verify that the block's remaining gas budget is large enough to
//! afford the BAL's remaining declared storage reads (`R_remaining * ITEM_COST`). If not, the
//! received BAL is adversarial — it declared reads it can't have actually performed — and we
//! reject before doing more EVM work.
//!
//! Intuition: every declared `storage_reads` slot costs at least `ITEM_COST` (2000 gas) when
//! actually read by the EVM. If `gas_remaining < R_remaining * ITEM_COST`, no honest
//! continuation of execution can satisfy the declaration, so the BAL must be wrong.
//!
//! The check is monotonic (gas used only grows; `r_seen` only grows), so running it after
//! every tx is strictly stronger than the EIP's every-8-txs cadence.
//!
//! ### Deriving `R_seen` without an inspector
//!
//! EIP-7928 requires `storage_reads` and `storage_changes` to be disjoint per account. So any
//! `(address, slot)` pair touched by a tx and *also* in the received BAL's declared
//! `storage_reads` set must be a pure read (not a write — writes appear in `storage_changes`,
//! a disjoint set). We scan `ResultAndState::state` for touched slots and intersect with the
//! declared-reads set; no revm inspector needed.

use super::{RejectReason, BAL_ITEM_COST};
use alloy_eip7928::AccountChanges;
use alloy_primitives::{Address, StorageKey, B256};
use revm::context::result::ResultAndState;
use std::collections::BTreeSet;

/// Tracks cumulative gas usage and distinct `storage_reads`-declared slots observed, and
/// enforces the feasibility invariant after each tx.
#[derive(Debug, Clone)]
pub struct FeasibilityTracker {
    /// Declared pure-read slots from the received BAL. `(addr, slot)` → membership.
    declared_reads: BTreeSet<(Address, StorageKey)>,
    /// Subset of `declared_reads` that has actually been touched by a worker's execution.
    /// Monotonically grows.
    r_seen: BTreeSet<(Address, StorageKey)>,
    /// `|declared_reads|` — derived once at construction for a cheap `r_remaining` computation.
    r_declared: u64,
    /// Cumulative gas used across all txs committed so far.
    gas_used: u64,
    /// Block gas limit.
    gas_limit: u64,
}

impl FeasibilityTracker {
    /// Derives `R_declared` and the declared-reads set from the received BAL.
    pub fn new(bal: &[AccountChanges], gas_limit: u64) -> Self {
        let declared_reads: BTreeSet<(Address, StorageKey)> = bal
            .iter()
            .flat_map(|acc| {
                let addr = acc.address;
                acc.storage_reads
                    .iter()
                    .map(move |slot| (addr, B256::from(slot.to_be_bytes::<32>())))
            })
            .collect();
        let r_declared = declared_reads.len() as u64;
        Self { declared_reads, r_seen: BTreeSet::new(), r_declared, gas_used: 0, gas_limit }
    }

    /// Records one tx's result and enforces feasibility: `gas_remaining >= r_remaining *
    /// ITEM_COST`.
    ///
    /// Generic over `HaltReason` so callers can pass any Evm's `ResultAndState` — the
    /// feasibility invariant only depends on `gas_used` and touched-slots from `state`,
    /// both of which are `HaltReason`-independent.
    pub fn record_and_check<H>(&mut self, result: &ResultAndState<H>) -> Result<(), RejectReason> {
        self.gas_used = self.gas_used.saturating_add(result.result.tx_gas_used());

        // Intersect this tx's touched slots with the declared-reads set.
        for (addr, account) in &result.state {
            for slot_u256 in account.storage.keys() {
                let slot = B256::from(slot_u256.to_be_bytes::<32>());
                let key = (*addr, slot);
                if self.declared_reads.contains(&key) {
                    self.r_seen.insert(key);
                }
            }
        }

        let r_remaining = self.r_declared.saturating_sub(self.r_seen.len() as u64);
        let gas_remaining = self.gas_limit.saturating_sub(self.gas_used);
        let needed = r_remaining.saturating_mul(BAL_ITEM_COST);

        if gas_remaining < needed {
            Err(RejectReason::FeasibilityCheckFailed {
                gas_remaining,
                reads_remaining: r_remaining,
            })
        } else {
            Ok(())
        }
    }

    /// Number of distinct declared-reads slots observed so far. Exposed for metrics/logging.
    #[inline]
    pub fn r_seen(&self) -> u64 {
        self.r_seen.len() as u64
    }

    /// Total declared-reads count. Fixed at construction.
    #[inline]
    pub const fn r_declared(&self) -> u64 {
        self.r_declared
    }

    /// Cumulative gas used. Exposed for metrics/logging.
    #[inline]
    pub const fn gas_used(&self) -> u64 {
        self.gas_used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::AccountChanges;
    use alloy_primitives::{map::HashMap as AlloyHashMap, U256};
    use revm::{
        context::result::{ExecutionResult, ResultGas, SuccessReason},
        state::{Account, AccountInfo, EvmStorageSlot},
    };

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    fn slot_u256(byte: u8) -> U256 {
        U256::from(byte)
    }

    /// Builds a minimal `ResultAndState` where `state` contains the specified touched slots.
    /// Gas-used is explicit.
    fn fake_tx_result(gas_used: u64, touches: &[(Address, U256)]) -> ResultAndState {
        let mut state = AlloyHashMap::default();
        for (addr, slot) in touches {
            let account = state.entry(*addr).or_insert_with(|| Account {
                info: AccountInfo::default(),
                original_info: Box::new(AccountInfo::default()),
                storage: Default::default(),
                status: Default::default(),
                transaction_id: 0,
            });
            account.storage.insert(*slot, EvmStorageSlot::new(U256::ZERO, 0));
        }

        ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Stop,
                // (total_gas_spent, refunded, floor_gas, state_gas_spent). tx_gas_used = total
                // - capped(refunded), so (gas_used, 0, 0, 0) yields tx_gas_used == gas_used.
                gas: ResultGas::new_with_state_gas(gas_used, 0, 0, 0),
                logs: Vec::new(),
                output: revm::context::result::Output::Call(Default::default()),
            },
            state,
        }
    }

    fn bal_with_reads(addr_byte: u8, slot_bytes: &[u8]) -> AccountChanges {
        AccountChanges {
            address: addr(addr_byte),
            storage_reads: slot_bytes.iter().map(|b| slot_u256(*b)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_bal_always_passes() {
        let bal: Vec<AccountChanges> = Vec::new();
        let mut t = FeasibilityTracker::new(&bal, 30_000_000);
        assert_eq!(t.record_and_check(&fake_tx_result(21_000, &[])), Ok(()));
        assert_eq!(t.r_declared(), 0);
    }

    #[test]
    fn touched_declared_read_increments_r_seen() {
        let bal = vec![bal_with_reads(1, &[10, 11])];
        let mut t = FeasibilityTracker::new(&bal, 30_000_000);
        assert_eq!(t.r_declared(), 2);

        // Touch slot 10 of addr(1).
        t.record_and_check(&fake_tx_result(21_000, &[(addr(1), slot_u256(10))])).unwrap();
        assert_eq!(t.r_seen(), 1);

        // Touch slot 11 too.
        t.record_and_check(&fake_tx_result(21_000, &[(addr(1), slot_u256(11))])).unwrap();
        assert_eq!(t.r_seen(), 2);
    }

    #[test]
    fn undeclared_touch_does_not_increment_r_seen() {
        let bal = vec![bal_with_reads(1, &[10])];
        let mut t = FeasibilityTracker::new(&bal, 30_000_000);
        // Touch a slot NOT declared in storage_reads.
        t.record_and_check(&fake_tx_result(21_000, &[(addr(1), slot_u256(99))])).unwrap();
        assert_eq!(t.r_seen(), 0);
    }

    #[test]
    fn repeat_touch_deduplicates() {
        let bal = vec![bal_with_reads(1, &[10])];
        let mut t = FeasibilityTracker::new(&bal, 30_000_000);
        t.record_and_check(&fake_tx_result(21_000, &[(addr(1), slot_u256(10))])).unwrap();
        t.record_and_check(&fake_tx_result(21_000, &[(addr(1), slot_u256(10))])).unwrap();
        assert_eq!(t.r_seen(), 1);
    }

    #[test]
    fn rejects_when_remaining_reads_exceed_remaining_gas() {
        // Declare 10 reads. Use 30M - 19_999 gas. Remaining gas = 19_999. Needed = 10 * 2000 =
        // 20_000. 19_999 < 20_000 → reject.
        let bal = vec![bal_with_reads(1, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])];
        let mut t = FeasibilityTracker::new(&bal, 30_000_000);
        let err = t.record_and_check(&fake_tx_result(30_000_000 - 19_999, &[])).unwrap_err();
        assert_eq!(
            err,
            RejectReason::FeasibilityCheckFailed { gas_remaining: 19_999, reads_remaining: 10 }
        );
    }

    #[test]
    fn boundary_gas_equals_needed_passes() {
        // 10 reads * 2000 = 20_000. gas_remaining == needed → OK (check is strict `<`).
        let bal = vec![bal_with_reads(1, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])];
        let mut t = FeasibilityTracker::new(&bal, 30_000_000);
        assert_eq!(t.record_and_check(&fake_tx_result(29_980_000, &[])), Ok(()));
    }

    #[test]
    fn observing_reads_reduces_needed_budget() {
        // Same setup as the reject test, but one declared read has been observed, so
        // r_remaining = 9 and needed = 18_000. 19_999 >= 18_000 → pass.
        let bal = vec![bal_with_reads(1, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])];
        let mut t = FeasibilityTracker::new(&bal, 30_000_000);
        t.record_and_check(&fake_tx_result(0, &[(addr(1), slot_u256(0))])).unwrap();
        assert_eq!(t.r_seen(), 1);
        assert_eq!(t.record_and_check(&fake_tx_result(30_000_000 - 19_999, &[])), Ok(()));
    }

    #[test]
    fn gas_used_saturates_not_panics_on_overflow() {
        let bal: Vec<AccountChanges> = Vec::new();
        let mut t = FeasibilityTracker::new(&bal, u64::MAX);
        // Would overflow u64 if not saturating.
        t.record_and_check(&fake_tx_result(u64::MAX, &[])).unwrap();
        t.record_and_check(&fake_tx_result(u64::MAX, &[])).unwrap();
        assert_eq!(t.gas_used(), u64::MAX);
    }
}
