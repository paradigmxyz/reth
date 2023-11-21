//! Helpers for working with EIP-4844 blob fee.

#[cfg(feature = "c-kzg")]
use crate::{constants::eip4844::VERSIONED_HASH_VERSION_KZG, B256};
#[cfg(feature = "c-kzg")]
use sha2::{Digest, Sha256};

// re-exports from revm for calculating blob fee
pub use crate::revm_primitives::{
    calc_blob_gasprice, calc_excess_blob_gas as calculate_excess_blob_gas,
};

/// Calculates the versioned hash for a KzgCommitment
///
/// Specified in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension)
#[cfg(feature = "c-kzg")]
pub fn kzg_to_versioned_hash(commitment: c_kzg::KzgCommitment) -> B256 {
    let mut res = Sha256::digest(commitment.as_slice());
    res[0] = VERSIONED_HASH_VERSION_KZG;
    B256::new(res.into())
}
