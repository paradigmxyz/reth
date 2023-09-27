//! Helpers for working with EIP-4844 blob fee
use crate::{
    constants::eip4844::{TARGET_DATA_GAS_PER_BLOCK, VERSIONED_HASH_VERSION_KZG},
    kzg::KzgCommitment,
    B256,
};
use sha2::{Digest, Sha256};

// re-exports from revm for calculating blob fee
pub use revm_primitives::calc_blob_gasprice;

/// Calculates the versioned hash for a KzgCommitment
///
/// Specified in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension)
pub fn kzg_to_versioned_hash(commitment: KzgCommitment) -> B256 {
    let mut res = Sha256::digest(commitment.as_slice());
    res[0] = VERSIONED_HASH_VERSION_KZG;
    B256::new(res.into())
}

/// Calculates the excess data gas for the next block, after applying the current set of blobs on
/// top of the excess data gas.
///
/// Specified in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension)
pub fn calculate_excess_blob_gas(parent_excess_blob_gas: u64, parent_blob_gas_used: u64) -> u64 {
    let excess_blob_gas = parent_excess_blob_gas + parent_blob_gas_used;
    excess_blob_gas.saturating_sub(TARGET_DATA_GAS_PER_BLOCK)
}
