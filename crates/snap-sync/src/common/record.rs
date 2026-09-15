//! Versioned progress records kept in the metadata table.
//!
//! [`read_record`] reports a record written by another build version instead of misreading it
//! when a download resumes.

use crate::SnapSyncError;
use reth_storage_api::{MetadataProvider, MetadataWriter};
use reth_storage_errors::provider::ProviderError;
use serde::{de::DeserializeOwned, Serialize};

/// Reads the record under `key`, reporting one written at another `version` instead of misreading
/// it.
pub(crate) fn read_record<T: DeserializeOwned>(
    provider: &impl MetadataProvider,
    key: &'static str,
    version: u32,
) -> Result<Option<T>, SnapSyncError> {
    let Some(bytes) = provider.get_metadata(key)? else { return Ok(None) };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(ProviderError::other)?;
    let found = value.get("version").and_then(serde_json::Value::as_u64);
    if found != Some(u64::from(version)) {
        return Err(SnapSyncError::UnsupportedRecord { key, version: found })
    }
    Ok(Some(serde_json::from_value(value).map_err(ProviderError::other)?))
}

/// Writes `record`, which carries its own `version` field, under `key`.
pub(crate) fn write_record(
    provider: &impl MetadataWriter,
    key: &str,
    record: &impl Serialize,
) -> Result<(), SnapSyncError> {
    provider.write_metadata(key, serde_json::to_vec(record).map_err(ProviderError::other)?)?;
    Ok(())
}
