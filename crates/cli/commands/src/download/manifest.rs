use eyre::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::info;

/// Default archive file names by convention.
///
/// When no manifest.json is available, component URLs are derived from a base URL using these
/// file names: `{base_url}/{file_name}`. This convention-based approach allows testing and
/// deployment without generating a manifest — just upload archives with the right names.
///
/// NOTE: A manifest.json-based approach may be adopted later for richer metadata (block number,
/// checksums, storage version). The convention-based approach is kept as the primary discovery
/// mechanism for simplicity and to enable local testing without manifest generation.
const CONVENTION_FILE_NAMES: &[(SnapshotComponentType, &str)] = &[
    (SnapshotComponentType::State, "state.tar.zst"),
    (SnapshotComponentType::Headers, "headers.tar.zst"),
    (SnapshotComponentType::Transactions, "transactions.tar.zst"),
    (SnapshotComponentType::Receipts, "receipts.tar.zst"),
    (SnapshotComponentType::AccountChangesets, "account-changesets.tar.zst"),
    (SnapshotComponentType::StorageChangesets, "storage-changesets.tar.zst"),
];

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

    /// Returns the conventional archive file name for this component.
    pub const fn default_file_name(&self) -> &'static str {
        match self {
            Self::State => "state.tar.zst",
            Self::Headers => "headers.tar.zst",
            Self::Transactions => "transactions.tar.zst",
            Self::Receipts => "receipts.tar.zst",
            Self::AccountChangesets => "account-changesets.tar.zst",
            Self::StorageChangesets => "storage-changesets.tar.zst",
        }
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

/// Per-component URL overrides. When set, these take precedence over convention-based discovery.
#[derive(Debug, Clone, Default)]
pub struct ComponentUrlOverrides {
    pub state: Option<String>,
    pub headers: Option<String>,
    pub transactions: Option<String>,
    pub receipts: Option<String>,
    pub account_changesets: Option<String>,
    pub storage_changesets: Option<String>,
}

impl ComponentUrlOverrides {
    /// Returns the override URL for a component type, if set.
    pub fn get(&self, ty: SnapshotComponentType) -> Option<&str> {
        match ty {
            SnapshotComponentType::State => self.state.as_deref(),
            SnapshotComponentType::Headers => self.headers.as_deref(),
            SnapshotComponentType::Transactions => self.transactions.as_deref(),
            SnapshotComponentType::Receipts => self.receipts.as_deref(),
            SnapshotComponentType::AccountChangesets => self.account_changesets.as_deref(),
            SnapshotComponentType::StorageChangesets => self.storage_changesets.as_deref(),
        }
    }

    /// Returns true if any override is set.
    pub fn has_any(&self) -> bool {
        self.state.is_some() ||
            self.headers.is_some() ||
            self.transactions.is_some() ||
            self.receipts.is_some() ||
            self.account_changesets.is_some() ||
            self.storage_changesets.is_some()
    }
}

/// Discovers available snapshot components using convention-based URL probing.
///
/// For each component type, constructs a URL as `{base_url}/{component}.tar.zst` and issues
/// a HEAD request to check availability and get the archive size. Components that return a
/// successful response are included in the manifest.
///
/// URL overrides take precedence: if an override URL is set for a component, that URL is probed
/// instead of the conventional one. This enables local testing with file:// URLs or custom
/// snapshot hosts.
///
/// Falls back to `manifest.json` if convention-based probing finds nothing (e.g. the snapshot
/// provider hasn't migrated to per-component archives yet).
pub async fn discover_components(
    base_url: &str,
    overrides: &ComponentUrlOverrides,
) -> Result<SnapshotManifest> {
    let client = Client::new();
    let mut components = BTreeMap::new();

    for &(ty, default_name) in CONVENTION_FILE_NAMES {
        let url = match overrides.get(ty) {
            Some(override_url) => override_url.to_string(),
            None => format!("{base_url}/{default_name}"),
        };

        // HEAD request to check existence and get size
        match client.head(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let size = resp.content_length().unwrap_or(0);
                info!(target: "reth::cli",
                    component = ty.display_name(),
                    url = %url,
                    size = %super::DownloadProgress::format_size(size),
                    "Found component"
                );
                components
                    .insert(ty.key().to_string(), SnapshotComponent { url, size, checksum: None });
            }
            Ok(resp) => {
                info!(target: "reth::cli",
                    component = ty.display_name(),
                    url = %url,
                    status = %resp.status(),
                    "Component not available"
                );
            }
            Err(e) => {
                info!(target: "reth::cli",
                    component = ty.display_name(),
                    url = %url,
                    error = %e,
                    "Failed to probe component"
                );
            }
        }
    }

    if components.is_empty() {
        // Fall back to manifest.json if no components found by convention
        let manifest_url = format!("{base_url}/manifest.json");
        if let Ok(resp) = client.get(&manifest_url).send().await {
            if resp.status().is_success() {
                let manifest: SnapshotManifest = resp.json().await?;
                info!(target: "reth::cli", "Loaded manifest.json fallback");
                return Ok(manifest);
            }
        }

        // Final fallback: legacy latest.txt
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
        let size = client
            .head(&archive_url)
            .send()
            .await
            .ok()
            .and_then(|r| r.content_length())
            .unwrap_or(0);

        components.insert(
            SnapshotComponentType::State.key().to_string(),
            SnapshotComponent { url: archive_url, size, checksum: None },
        );
    }

    Ok(SnapshotManifest {
        block: 0,
        chain_id: 0,
        storage_version: String::new(),
        timestamp: 0,
        components,
    })
}
