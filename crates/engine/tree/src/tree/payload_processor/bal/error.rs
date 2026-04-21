//! Rejection reasons for the BAL execution path.

use alloy_primitives::B256;

/// Reasons a block may be rejected on the BAL execution path.
///
/// These correspond to the validation stages described in `BAL.md`. Each stage maps to a specific
/// variant; producing an instance short-circuits block validation and propagates as an invalid
/// block error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// `keccak256(rlp(received_bal))` disagrees with `header.block_access_list_hash`.
    HeaderHashMismatch {
        /// Hash computed from the received BAL bytes.
        computed: B256,
        /// Hash committed in the block header.
        expected: B256,
    },

    /// BAL fails the structural item-count gate:
    /// `(addresses + unique_storage_keys) * ITEM_COST > block_gas_limit`.
    ItemCountExceedsGasBudget {
        /// Count of addresses + unique storage keys.
        bal_items: u64,
        /// Block gas limit in gas units.
        gas_limit: u64,
    },

    /// A worker accessed state not declared in the received BAL (surfaced as revm's
    /// `BalError::AccountNotFound` or a storage-key miss).
    ///
    /// TODO: once revm's `BalError` carries the offending `address` / `slot`, thread those
    /// through to aid diagnostics. For now we only know which tx triggered the miss.
    UndeclaredAccess {
        /// Tx index whose worker reported the miss.
        tx_index: u64,
    },

    /// The phantom-reads feasibility invariant failed:
    /// `G_remaining < R_remaining * ITEM_COST` after some tx completed.
    ///
    /// See <https://ethresear.ch/t/early-rejection-of-adversarial-bals/23995>.
    FeasibilityCheckFailed {
        /// Block gas remaining at the time of the check.
        gas_remaining: u64,
        /// Declared storage reads not yet observed in any worker's execution.
        reads_remaining: u64,
    },

    /// The canonical `bal_builder`'s entries for a tx disagree with the received BAL for the same
    /// tx index. Caught per-tx immediately after `commit_transaction`.
    FragmentMismatch {
        /// Tx index whose fragment diverged.
        tx_index: u64,
    },

    /// The rebuilt BAL's hash disagrees with `header.block_access_list_hash` at end-of-block.
    /// Catches over-declared addresses and any divergence the per-tx fragment compares missed.
    FinalHashMismatch {
        /// Hash of the rebuilt BAL.
        rebuilt: B256,
        /// Hash committed in the block header.
        expected: B256,
    },
}
