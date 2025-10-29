//! Metadata provider trait for reading and writing node metadata.

use reth_codecs::Compact;
use reth_db_api::models::StorageSettings;
use reth_storage_errors::provider::ProviderResult;

/// Metadata keys.
pub mod keys {
    /// Storage configuration settings for this node.
    pub const STORAGE_SETTINGS: &str = "storage_settings";
}

/// Client trait for reading node metadata from the database.
#[auto_impl::auto_impl(&, Arc)]
pub trait MetadataProvider: Send + Sync {
    /// Get a metadata value by key
    fn get_metadata(&self, key: &str) -> ProviderResult<Option<Vec<u8>>>;

    /// Get storage settings for this node
    fn storage_settings(&self) -> ProviderResult<Option<StorageSettings>> {
        Ok(self
            .get_metadata(keys::STORAGE_SETTINGS)?
            .map(|bytes| StorageSettings::from_compact(&bytes, bytes.len()).0))
    }
}

/// Client trait for writing node metadata to the database.
pub trait MetadataWriter: Send + Sync {
    /// Write a metadata value
    fn write_metadata(&self, key: &str, value: Vec<u8>) -> ProviderResult<()>;

    /// Write storage settings for this node
    ///
    /// Be sure to update provider factory cache with
    /// [`StorageSettingsCache::set_storage_settings_cache`].
    fn write_storage_settings(&self, settings: StorageSettings) -> ProviderResult<()> {
        use reth_codecs::Compact;
        let mut buf = Vec::new();
        settings.to_compact(&mut buf);
        self.write_metadata(keys::STORAGE_SETTINGS, buf)
    }
}

/// Trait for caching storage settings on a provider factory.
pub trait StorageSettingsCache: Send + Sync {
    /// Sets the storage settings of this `ProviderFactory`.
    ///
    /// IMPORTANT: It does not save settings in storage, that should be done by
    /// [`MetadataWriter::write_storage_settings`]
    fn set_storage_settings_cache(&self, settings: StorageSettings);
}
