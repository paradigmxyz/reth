//! Rejection reasons for the BAL execution path.

use alloy_primitives::B256;

/// Reasons a block may be rejected on the BAL execution path.
///
/// These correspond to the validation stages described in `BAL.md`. Each stage maps to a specific
/// variant; producing an instance short-circuits block validation and propagates as an invalid
/// block error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
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

    /// The rebuilt BAL's hash disagrees with `header.block_access_list_hash` at end-of-block.
    /// Catches over-declared addresses and any divergence the per-tx fragment compares missed.
    FinalHashMismatch {
        /// Hash of the rebuilt BAL.
        rebuilt: B256,
        /// Hash committed in the block header.
        expected: B256,
    },
}
