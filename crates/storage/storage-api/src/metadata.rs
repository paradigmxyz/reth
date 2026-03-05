//! Metadata provider trait for reading and writing node metadata.

use alloc::vec::Vec;
use reth_db_api::models::StorageSettings;
use reth_storage_errors::provider::{ProviderError, ProviderResult};

/// Metadata keys.
pub mod keys {
    /// Storage configuration settings for this node.
    pub const STORAGE_SETTINGS: &str = "storage_settings";
}

/// Client trait for reading node metadata from the database.
#[auto_impl::auto_impl(&, Arc)]
pub trait MetadataProvider: Send {
    /// Get a metadata value by key
    fn get_metadata(&self, key: &str) -> ProviderResult<Option<Vec<u8>>>;

    /// Get storage settings for this node.
    ///
    /// If the stored metadata can't be deserialized (e.g. the format changed),
    /// this returns `None` instead of an error so commands like `db clear` can
    /// still operate without requiring a compatible metadata schema.
    fn storage_settings(&self) -> ProviderResult<Option<StorageSettings>> {
        Ok(self
            .get_metadata(keys::STORAGE_SETTINGS)?
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
    }
}

/// Client trait for writing node metadata to the database.
pub trait MetadataWriter: Send {
    /// Write a metadata value
    fn write_metadata(&self, key: &str, value: Vec<u8>) -> ProviderResult<()>;

    /// Write storage settings for this node
    ///
    /// Be sure to update provider factory cache with
    /// [`StorageSettingsCache::set_storage_settings_cache`].
    fn write_storage_settings(&self, settings: StorageSettings) -> ProviderResult<()> {
        self.write_metadata(
            keys::STORAGE_SETTINGS,
            serde_json::to_vec(&settings).map_err(ProviderError::other)?,
        )
    }
}

/// Trait for caching storage settings on a provider factory.
pub trait StorageSettingsCache: Send {
    /// Gets the cached storage settings.
    fn cached_storage_settings(&self) -> StorageSettings;

    /// Sets the storage settings of this `ProviderFactory`.
    ///
    /// IMPORTANT: It does not save settings in storage, that should be done by
    /// [`MetadataWriter::write_storage_settings`]
    fn set_storage_settings_cache(&self, settings: StorageSettings);
}

/// Trait for accessing the database directory path.
#[cfg(feature = "std")]
pub trait StoragePath: Send {
    /// Returns the path to the database directory (e.g. `<datadir>/db`).
    fn storage_path(&self) -> std::path::PathBuf;
}
