//! Versioned progress records kept in the metadata table.
//!
//! Account coverage and storage progress each implement [`SnapRecord`], so a record written by
//! another build version is reported instead of misread when a download resumes.

use crate::SnapSyncError;
use reth_storage_api::{MetadataProvider, MetadataWriter};
use reth_storage_errors::provider::ProviderError;
use serde::{de::DeserializeOwned, Serialize};

/// A progress record stored as JSON under its own metadata key.
///
/// The serialized record carries a `version` field, checked before the rest is decoded.
pub(crate) trait SnapRecord: Serialize + DeserializeOwned {
    /// Metadata key the record is stored under.
    const KEY: &'static str;

    /// Encoding version this build writes.
    const VERSION: u32;

    /// Reads the record, reporting one written at another version instead of misreading it.
    fn read(provider: &impl MetadataProvider) -> Result<Option<Self>, SnapSyncError> {
        let Some(bytes) = provider.get_metadata(Self::KEY)? else { return Ok(None) };
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(ProviderError::other)?;
        let version = value.get("version").and_then(serde_json::Value::as_u64);
        if version != Some(u64::from(Self::VERSION)) {
            return Err(SnapSyncError::UnsupportedRecord { key: Self::KEY, version })
        }
        Ok(Some(serde_json::from_value(value).map_err(ProviderError::other)?))
    }

    /// Writes this record under its key.
    fn write(&self, provider: &impl MetadataWriter) -> Result<(), SnapSyncError> {
        provider
            .write_metadata(Self::KEY, serde_json::to_vec(self).map_err(ProviderError::other)?)?;
        Ok(())
    }
}
