//! Worker scaling utilities for parallel trie computation.
//!
//! Provides functions to compute desired worker counts based on
//! the previous block's state size (accounts + storage slots).

pub use reth_trie_common::WorkloadHint;

/// Number of storage slots per worker for scaling calculations.
pub const SLOTS_PER_WORKER: usize = 50;

/// Number of accounts per worker for scaling calculations.
pub const ACCOUNTS_PER_WORKER: usize = 20;

/// Computes desired worker counts based on previous block's workload.
///
/// Returns `(storage_workers, account_workers)`.
///
/// If `hint` is `None` or empty, returns `(min, min)`.
pub fn desired_workers(
    hint: Option<WorkloadHint>,
    min_storage: usize,
    max_storage: usize,
    min_account: usize,
    max_account: usize,
) -> (usize, usize) {
    let Some(hint) = hint else {
        return (min_storage, min_account);
    };

    let desired_storage = if hint.storage_slots == 0 {
        min_storage
    } else {
        hint.storage_slots.div_ceil(SLOTS_PER_WORKER).clamp(min_storage, max_storage)
    };

    let desired_account = if hint.accounts == 0 {
        min_account
    } else {
        hint.accounts.div_ceil(ACCOUNTS_PER_WORKER).clamp(min_account, max_account)
    };

    (desired_storage, desired_account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_hint_returns_min() {
        let (storage, account) = desired_workers(None, 4, 64, 2, 32);
        assert_eq!(storage, 4);
        assert_eq!(account, 2);
    }

    #[test]
    fn test_empty_hint_returns_min() {
        let hint = WorkloadHint::default();
        let (storage, account) = desired_workers(Some(hint), 4, 64, 2, 32);
        assert_eq!(storage, 4); // storage slots == 0 → min_storage
        assert_eq!(account, 2); // accounts == 0 → min_account
    }

    #[test]
    fn test_scaling_formula() {
        // 100 accounts, 500 storage slots
        let hint = WorkloadHint::new(100, 500);
        let (storage_workers, account_workers) = desired_workers(Some(hint), 4, 64, 2, 32);

        // 500 slots / 50 = 10 storage workers
        assert_eq!(storage_workers, 10);
        // 100 accounts / 20 = 5 account workers
        assert_eq!(account_workers, 5);
    }

    #[test]
    fn test_clamp_to_max() {
        // 10000 accounts (would need 500 workers unclamped)
        let hint = WorkloadHint::new(10000, 0);
        let (_, account_workers) = desired_workers(Some(hint), 2, 32, 2, 32);
        assert_eq!(account_workers, 32); // Clamped to max
    }

}
