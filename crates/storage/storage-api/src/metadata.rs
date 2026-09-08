//! Metadata provider trait for reading and writing node metadata.

use alloc::vec::Vec;
use core::fmt;
use reth_db_api::models::{SnapAttempt, SnapAttemptId, StorageSettings, SNAP_ATTEMPT_VERSION};
use reth_storage_errors::provider::{ProviderError, ProviderResult};

/// Metadata keys.
pub mod keys {
    /// Storage configuration settings for this node.
    pub const STORAGE_SETTINGS: &str = "storage_settings";

    /// The snap synchronization attempt that owns downloaded state.
    pub const SNAP_ATTEMPT: &str = "snap_attempt";

    /// Identity the next snap attempt takes.
    ///
    /// Kept apart from the record so clearing an attempt cannot hand its identity to a later one
    /// while the cleared attempt's downloads are still outstanding.
    pub const SNAP_ATTEMPT_NEXT_ID: &str = "snap_attempt_next_id";
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

    /// Returns the snap synchronization attempt that owns the downloaded state.
    ///
    /// Unlike [`Self::storage_settings`], an unreadable record is an error: its state is already
    /// in the canonical tables, so reporting it absent would let the node adopt it.
    fn snap_attempt(&self) -> ProviderResult<Option<SnapAttempt>> {
        let Some(bytes) = self.get_metadata(keys::SNAP_ATTEMPT)? else { return Ok(None) };

        // Read the version first, so a future build's record is reported as unsupported rather
        // than as a decode failure.
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(ProviderError::other)?;
        let found = value.get("version").and_then(serde_json::Value::as_u64);
        if found != Some(SNAP_ATTEMPT_VERSION as u64) {
            return Err(ProviderError::other(UnsupportedSnapAttemptVersion {
                found,
                supported: SNAP_ATTEMPT_VERSION,
            }))
        }

        serde_json::from_slice(&bytes).map(Some).map_err(ProviderError::other)
    }

    /// Returns the identity the next snap attempt takes, which only ever increases.
    fn snap_attempt_next_id(&self) -> ProviderResult<SnapAttemptId> {
        let Some(bytes) = self.get_metadata(keys::SNAP_ATTEMPT_NEXT_ID)? else {
            return Ok(SnapAttemptId::FIRST)
        };
        serde_json::from_slice(&bytes).map_err(ProviderError::other)
    }
}

/// A persisted [`SnapAttempt`] record this build cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedSnapAttemptVersion {
    // Version found on disk, absent when the record carries no numeric version.
    found: Option<u64>,
    // Version this build writes.
    supported: u32,
}

impl fmt::Display for UnsupportedSnapAttemptVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { found, supported } = self;
        match found {
            Some(found) => write!(f, "snap attempt record version {found} is not supported (this build writes {supported})"),
            None => write!(f, "snap attempt record has no version (this build writes {supported})"),
        }
    }
}

impl core::error::Error for UnsupportedSnapAttemptVersion {}

/// Client trait for writing node metadata to the database.
pub trait MetadataWriter: Send {
    /// Write a metadata value
    fn write_metadata(&self, key: &str, value: Vec<u8>) -> ProviderResult<()>;

    /// Delete a metadata value.
    fn delete_metadata(&self, _key: &str) -> ProviderResult<()> {
        Err(ProviderError::UnsupportedProvider)
    }

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

    /// Writes the snap synchronization attempt that owns the downloaded state.
    fn write_snap_attempt(&self, attempt: &SnapAttempt) -> ProviderResult<()> {
        self.write_metadata(
            keys::SNAP_ATTEMPT,
            serde_json::to_vec(attempt).map_err(ProviderError::other)?,
        )
    }

    /// Writes the identity the next snap attempt takes.
    fn write_snap_attempt_next_id(&self, id: SnapAttemptId) -> ProviderResult<()> {
        self.write_metadata(
            keys::SNAP_ATTEMPT_NEXT_ID,
            serde_json::to_vec(&id).map_err(ProviderError::other)?,
        )
    }

    /// Removes the record, leaving the next identity untouched so it cannot be reused.
    fn clear_snap_attempt(&self) -> ProviderResult<()> {
        self.delete_metadata(keys::SNAP_ATTEMPT)
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
