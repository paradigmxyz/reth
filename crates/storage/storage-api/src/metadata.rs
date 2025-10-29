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
    fn write_storage_settings(&self, settings: StorageSettings) -> ProviderResult<()> {
        let mut buf = Vec::new();
        settings.to_compact(&mut buf);
        self.write_metadata(keys::STORAGE_SETTINGS, buf)
    }
}
