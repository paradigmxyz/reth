//! [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#parameters) protocol constants and utils for shard Blob Transactions.
#[cfg(all(feature = "c-kzg", feature = "std"))]
pub use trusted_setup::*;

pub use alloy_eips::eip4844::{
    BLOB_GASPRICE_UPDATE_FRACTION, BLOB_TX_MIN_BLOB_GASPRICE, DATA_GAS_PER_BLOB,
    FIELD_ELEMENTS_PER_BLOB, FIELD_ELEMENT_BYTES, MAX_BLOBS_PER_BLOCK, MAX_DATA_GAS_PER_BLOCK,
    TARGET_BLOBS_PER_BLOCK, TARGET_DATA_GAS_PER_BLOCK, VERSIONED_HASH_VERSION_KZG,
};

// These 2 to silence unused
#[cfg(all(feature = "c-kzg", not(feature = "std")))]
use tempfile as _;
#[cfg(all(not(feature = "c-kzg"), feature = "std"))]
use thiserror as _;

#[cfg(all(feature = "c-kzg", feature = "std"))]
mod trusted_setup {
    use crate::kzg::KzgSettings;
    use std::io::Write;

    /// Loads the trusted setup parameters from the given bytes and returns the [`KzgSettings`].
    ///
    /// This creates a temp file to store the bytes and then loads the [`KzgSettings`] from the file
    /// via [`KzgSettings::load_trusted_setup_file`].
    pub fn load_trusted_setup_from_bytes(
        bytes: &[u8],
    ) -> Result<KzgSettings, LoadKzgSettingsError> {
        let mut file = tempfile::NamedTempFile::new().map_err(LoadKzgSettingsError::TempFileErr)?;
        file.write_all(bytes).map_err(LoadKzgSettingsError::TempFileErr)?;
        KzgSettings::load_trusted_setup_file(file.path()).map_err(LoadKzgSettingsError::KzgError)
    }

    /// Error type for loading the trusted setup.
    #[derive(Debug, thiserror::Error)]
    pub enum LoadKzgSettingsError {
        /// Failed to create temp file to store bytes for loading [`KzgSettings`] via
        /// [`KzgSettings::load_trusted_setup_file`].
        #[error("failed to setup temp file: {0}")]
        TempFileErr(#[from] std::io::Error),
        /// Kzg error
        #[error("KZG error: {0:?}")]
        KzgError(#[from] c_kzg::Error),
    }
}
