//! [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#parameters) protocol constants and utils for shard Blob Transactions.

use crate::{kzg::KzgSettings, U128};
use once_cell::sync::Lazy;
use std::{io::Write, sync::Arc};

/// Size a single field element in bytes.
pub const FIELD_ELEMENT_BYTES: u64 = 32;

/// How many field elements are stored in a single data blob.
pub const FIELD_ELEMENTS_PER_BLOB: u64 = 4096;

/// Gas consumption of a single data blob.
pub const DATA_GAS_PER_BLOB: u64 = 131_072u64; // 32*4096 = 131072 == 2^17 == 0x20000

/// Maximum data gas for data blobs in a single block.
pub const MAX_DATA_GAS_PER_BLOCK: u64 = 786_432u64; // 0xC0000

/// Target data gas for data blobs in a single block.
pub const TARGET_DATA_GAS_PER_BLOCK: u64 = 393_216u64; // 0x60000

/// Maximum number of data blobs in a single block.
pub const MAX_BLOBS_PER_BLOCK: usize = (MAX_DATA_GAS_PER_BLOCK / DATA_GAS_PER_BLOB) as usize; // 786432 / 131072  = 6

/// Target number of data blobs in a single block.
pub const TARGET_BLOBS_PER_BLOCK: u64 = TARGET_DATA_GAS_PER_BLOCK / DATA_GAS_PER_BLOB; // 393216 / 131072 = 3

/// Determines the maximum rate of change for blob fee
pub const BLOB_GASPRICE_UPDATE_FRACTION: u64 = 3_338_477u64; // 3338477

/// Minimum gas price for a data blob
pub const BLOB_TX_MIN_BLOB_GASPRICE: u64 = 1u64;

/// Commitment version of a KZG commitment
pub const VERSIONED_HASH_VERSION_KZG: u8 = 0x01;

/// KZG Trusted setup raw
const TRUSTED_SETUP_RAW: &[u8] = include_bytes!("../../res/eip4844/trusted_setup.txt");

/// KZG trusted setup
pub static MAINNET_KZG_TRUSTED_SETUP: Lazy<Arc<KzgSettings>> = Lazy::new(|| {
    Arc::new(
        load_trusted_setup_from_bytes(TRUSTED_SETUP_RAW).expect("Failed to load trusted setup"),
    )
});

/// Loads the trusted setup parameters from the given bytes and returns the [KzgSettings].
///
/// This creates a temp file to store the bytes and then loads the [KzgSettings] from the file via
/// [KzgSettings::load_trusted_setup_file].
pub fn load_trusted_setup_from_bytes(bytes: &[u8]) -> Result<KzgSettings, LoadKzgSettingsError> {
    let mut file = tempfile::NamedTempFile::new().map_err(LoadKzgSettingsError::TempFileErr)?;
    file.write_all(bytes).map_err(LoadKzgSettingsError::TempFileErr)?;
    KzgSettings::load_trusted_setup_file(file.path()).map_err(LoadKzgSettingsError::KzgError)
}

/// Error type for loading the trusted setup.
#[derive(Debug, thiserror::Error)]
pub enum LoadKzgSettingsError {
    /// Failed to create temp file to store bytes for loading [KzgSettings] via
    /// [KzgSettings::load_trusted_setup_file].
    #[error("Failed to setup temp file: {0:?}")]
    TempFileErr(#[from] std::io::Error),
    /// Kzg error
    #[error("Kzg error: {0:?}")]
    KzgError(c_kzg::Error),
}

/// Calculates the blob fee for the given excess blob gas.
pub fn blob_fee(excess_blob_gas: u64) -> U128 {
    fake_exponential(
        U128::from(BLOB_TX_MIN_BLOB_GASPRICE),
        U128::from(excess_blob_gas),
        U128::from(BLOB_GASPRICE_UPDATE_FRACTION),
    )
}

/// Approximates factor * e ** (numerator / denominator) using Taylor expansion.
///
/// This is used to calculate the blob price.
///
/// See also <https://eips.ethereum.org/EIPS/eip-4844#helpers>
pub fn fake_exponential(factor: U128, numerator: U128, denominator: U128) -> U128 {
    let mut output = U128::ZERO;
    let mut numerator_accum = factor.saturating_mul(denominator);
    let mut i = U128::from(1u64);
    while numerator_accum > U128::ZERO {
        output += numerator_accum;
        numerator_accum = numerator_accum * numerator / (denominator * i);
        i += U128::from(1u64);
    }
    output / denominator
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_load_kzg_settings() {
        let _settings = Arc::clone(&MAINNET_KZG_TRUSTED_SETUP);
    }

    #[test]
    fn test_fake_exp() {
        // <https://github.com/ethereum/go-ethereum/blob/28857080d732857030eda80c69b9ba2c8926f221/consensus/misc/eip4844/eip4844_test.go#L78-L78>
        for (factor, num, denom, expected) in &[
            (1u64, 0u64, 1u64, 1u64),
            (38493, 0, 1000, 38493),
            (0, 1234, 2345, 0),
            (1, 2, 1, 6), // approximate 7.389
            (1, 4, 2, 6),
            (1, 3, 1, 16), // approximate 20.09
            (1, 6, 2, 18),
            (1, 4, 1, 49), // approximate 54.60
            (1, 8, 2, 50),
            (10, 8, 2, 542), // approximate 540.598
            (11, 8, 2, 596), // approximate 600.58
            (1, 5, 1, 136),  // approximate 148.4
            (1, 5, 2, 11),   // approximate 12.18
            (2, 5, 2, 23),   // approximate 24.36
            (1, 50000000, 2225652, 5709098764),
        ] {
            let res = fake_exponential(U128::from(*factor), U128::from(*num), U128::from(*denom));
            assert_eq!(res, U128::from(*expected));
        }
    }
}
