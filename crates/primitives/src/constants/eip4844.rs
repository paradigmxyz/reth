//! [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#parameters) protocol constants and utils for shard Blob Transactions.

use crate::kzg::KzgSettings;
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
pub const BLOB_TX_MIN_BLOB_GASPRICE: u128 = 1u128;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_load_kzg_settings() {
        let _settings = Arc::clone(&MAINNET_KZG_TRUSTED_SETUP);
    }
}
