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

    /// Get storage settings for this node in a lossy/compatibility mode.
    ///
    /// If the stored metadata can't be deserialized (e.g. the format changed),
    /// this returns `None` instead of an error so tooling commands (for example
    /// `db clear`) can still operate without requiring a compatible metadata schema.
    ///
    /// Node startup paths that require strict correctness should use
    /// [`Self::strict_storage_settings`].
    fn storage_settings(&self) -> ProviderResult<Option<StorageSettings>> {
        Ok(self
            .get_metadata(keys::STORAGE_SETTINGS)?
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
    }

    /// Get storage settings for this node in strict mode.
    ///
    /// In contrast to [`Self::storage_settings`], deserialization failures are
    /// returned as errors instead of silently being mapped to `None`.
    fn strict_storage_settings(&self) -> ProviderResult<Option<StorageSettings>> {
        self.get_metadata(keys::STORAGE_SETTINGS)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(ProviderError::other))
            .transpose()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestMetadataProvider {
        value: Option<Vec<u8>>,
    }

    impl MetadataProvider for TestMetadataProvider {
        fn get_metadata(&self, _key: &str) -> ProviderResult<Option<Vec<u8>>> {
            Ok(self.value.clone())
        }
    }

    #[test]
    fn lossy_storage_settings_ignores_deserialization_error() {
        let provider = TestMetadataProvider { value: Some(b"{invalid json".to_vec()) };

        assert_eq!(provider.storage_settings().unwrap(), None);
    }

    #[test]
    fn strict_storage_settings_surfaces_deserialization_error() {
        let provider = TestMetadataProvider { value: Some(b"{invalid json".to_vec()) };

        let err = provider.strict_storage_settings().unwrap_err();
        assert!(err.is_other::<serde_json::Error>());
    }
}
