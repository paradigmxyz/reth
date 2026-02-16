use eyre::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A snapshot manifest describes available components for a snapshot at a given block height.
///
/// Each component is a separate archive that can be independently downloaded. The `state`
/// component is always required; other components (headers, transactions, receipts, changesets)
/// are optional and can be selected by the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Block number this snapshot was taken at.
    pub block: u64,
    /// Chain ID.
    pub chain_id: u64,
    /// Storage version (e.g. "v2").
    pub storage_version: String,
    /// Timestamp when the snapshot was created (unix seconds).
    pub timestamp: u64,
    /// Available snapshot components keyed by [`SnapshotComponentType`] name.
    pub components: BTreeMap<String, SnapshotComponent>,
}

/// A single downloadable snapshot component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotComponent {
    /// Download URL for this component's archive.
    pub url: String,
    /// Compressed archive size in bytes.
    pub size: u64,
    /// Optional SHA-256 checksum of the archive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

/// The types of snapshot components that can be downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapshotComponentType {
    /// State database (mdbx + rocksdb). Always required.
    State,
    /// Block headers static files.
    Headers,
    /// Transaction static files.
    Transactions,
    /// Receipt static files.
    Receipts,
    /// Account changeset static files.
    AccountChangesets,
    /// Storage changeset static files.
    StorageChangesets,
}

impl SnapshotComponentType {
    /// All component types in display order.
    pub const ALL: [Self; 6] = [
        Self::State,
        Self::Headers,
        Self::Transactions,
        Self::Receipts,
        Self::AccountChangesets,
        Self::StorageChangesets,
    ];

    /// The string key used in the manifest JSON.
    pub const fn key(&self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Headers => "headers",
            Self::Transactions => "transactions",
            Self::Receipts => "receipts",
            Self::AccountChangesets => "account_changesets",
            Self::StorageChangesets => "storage_changesets",
        }
    }

    /// Human-readable display name.
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::State => "State (mdbx + rocksdb)",
            Self::Headers => "Headers",
            Self::Transactions => "Transactions",
            Self::Receipts => "Receipts",
            Self::AccountChangesets => "Account Changesets",
            Self::StorageChangesets => "Storage Changesets",
        }
    }

    /// Whether this component is always required.
    pub const fn is_required(&self) -> bool {
        matches!(self, Self::State)
    }
}

impl SnapshotManifest {
    /// Look up a component by type.
    pub fn component(&self, ty: SnapshotComponentType) -> Option<&SnapshotComponent> {
        self.components.get(ty.key())
    }

    /// Returns the total download size for the given set of component types.
    pub fn total_size(&self, types: &[SnapshotComponentType]) -> u64 {
        types.iter().filter_map(|ty| self.component(*ty).map(|c| c.size)).sum()
    }
}

/// Fetch a snapshot manifest from a base URL.
///
/// Tries `{base_url}/manifest.json` first. If not found, falls back to the legacy
/// `latest.txt` single-archive flow by constructing a synthetic manifest with a single
/// `state` component.
pub async fn fetch_manifest(base_url: &str) -> Result<SnapshotManifest> {
    let client = Client::new();
    let manifest_url = format!("{base_url}/manifest.json");

    match client.get(&manifest_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let manifest: SnapshotManifest = resp.json().await?;
            Ok(manifest)
        }
        _ => {
            // Fall back to legacy latest.txt flow
            let latest_url = format!("{base_url}/latest.txt");
            let filename = client
                .get(&latest_url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?
                .trim()
                .to_string();

            let archive_url = format!("{base_url}/{filename}");

            // HEAD request to get the archive size
            let size = client
                .head(&archive_url)
                .send()
                .await
                .ok()
                .and_then(|r| r.content_length())
                .unwrap_or(0);

            let mut components = BTreeMap::new();
            components.insert(
                SnapshotComponentType::State.key().to_string(),
                SnapshotComponent { url: archive_url, size, checksum: None },
            );

            Ok(SnapshotManifest {
                block: 0,
                chain_id: 0,
                storage_version: "unknown".to_string(),
                timestamp: 0,
                components,
            })
        }
    }
}
