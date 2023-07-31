//! Helpers for working with EIP-4844 blob fee

use crate::constants::eip4844::TARGET_DATA_GAS_PER_BLOCK;

/// Calculates the excess data gas for the next block, after applying the current set of blobs on
/// top of the excess data gas.
///
/// Specified in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension)
pub fn calculate_excess_blob_gas(parent_excess_blob_gas: u64, parent_blob_gas_used: u64) -> u64 {
    let excess_blob_gas = parent_excess_blob_gas + parent_blob_gas_used;
    excess_blob_gas.saturating_sub(TARGET_DATA_GAS_PER_BLOCK)
}
