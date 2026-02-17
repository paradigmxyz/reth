use eyre::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
use tracing::info;

/// A snapshot manifest describes available components for a snapshot at a given block height.
///
/// Each component is either a single archive (state) or a set of chunked archives (static file
/// segments like transactions, receipts, etc). Chunked components use `blocks_per_file` to
/// define the block range per archive, matching reth's static file segment boundaries.
///
/// Archive naming convention for chunked components:
///   `{component}-{start_block}-{end_block}.tar.zst`
///
/// For example with `blocks_per_file: 500000` and `total_blocks: 1500000`:
///   `transactions-0-499999.tar.zst`
///   `transactions-500000-999999.tar.zst`
///   `transactions-1000000-1499999.tar.zst`
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
    /// Base URL for archive downloads. Component archive URLs are relative to this.
    pub base_url: String,
    /// Available snapshot components.
    pub components: BTreeMap<String, ComponentManifest>,
}

/// Manifest entry for a single snapshot component.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComponentManifest {
    /// A single archive file (used for state).
    Single(SingleArchive),
    /// A set of chunked archives split by block range (used for static file segments).
    Chunked(ChunkedArchive),
}

/// A single, non-chunked archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleArchive {
    /// Archive file name (relative to base_url).
    pub file: String,
    /// Compressed archive size in bytes.
    pub size: u64,
    /// Optional SHA-256 checksum.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

/// A chunked archive set where each chunk covers a fixed block range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkedArchive {
    /// Number of blocks per archive file. Matches reth's `blocks_per_file` config.
    pub blocks_per_file: u64,
    /// Total number of blocks covered by this component.
    pub total_blocks: u64,
}

/// The types of snapshot components that can be downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapshotComponentType {
    /// State database (mdbx). Always required. Single archive.
    State,
    /// Index database (rocksdb). Optional — present in archive snapshots. Single archive.
    Indexes,
    /// Block headers static files. Chunked.
    Headers,
    /// Transaction static files. Chunked.
    Transactions,
    /// Receipt static files. Chunked.
    Receipts,
    /// Account changeset static files. Chunked.
    AccountChangesets,
    /// Storage changeset static files. Chunked.
    StorageChangesets,
}

impl SnapshotComponentType {
    /// All component types in display order.
    pub const ALL: [Self; 7] = [
        Self::State,
        Self::Indexes,
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
            Self::Indexes => "indexes",
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
            Self::State => "State (mdbx)",
            Self::Indexes => "Indexes (rocksdb)",
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

    /// Whether this component type uses chunked archives.
    pub const fn is_chunked(&self) -> bool {
        !matches!(self, Self::State | Self::Indexes)
    }
}

impl SnapshotManifest {
    /// Look up a component by type.
    pub fn component(&self, ty: SnapshotComponentType) -> Option<&ComponentManifest> {
        self.components.get(ty.key())
    }

    /// Returns the total download size for the given set of component types.
    pub fn total_size(&self, types: &[SnapshotComponentType]) -> u64 {
        types.iter().filter_map(|ty| self.component(*ty).map(|c| c.total_size())).sum()
    }

    /// Returns all archive URLs for a given component type.
    pub fn archive_urls(&self, ty: SnapshotComponentType) -> Vec<String> {
        let Some(component) = self.component(ty) else {
            return vec![];
        };

        match component {
            ComponentManifest::Single(single) => {
                vec![format!("{}/{}", self.base_url, single.file)]
            }
            ComponentManifest::Chunked(chunked) => {
                let key = ty.key();
                let num_chunks = chunked.num_chunks();
                (0..num_chunks)
                    .map(|i| {
                        let start = i * chunked.blocks_per_file;
                        let end = ((i + 1) * chunked.blocks_per_file).min(chunked.total_blocks) - 1;
                        format!("{}/{key}-{start}-{end}.tar.zst", self.base_url)
                    })
                    .collect()
            }
        }
    }
}

impl ComponentManifest {
    /// Returns the total download size for this component.
    pub fn total_size(&self) -> u64 {
        match self {
            Self::Single(s) => s.size,
            // Individual chunk sizes are discovered at download time
            Self::Chunked(_) => 0,
        }
    }
}

impl ChunkedArchive {
    /// Returns the number of chunks.
    pub fn num_chunks(&self) -> u64 {
        self.total_blocks.div_ceil(self.blocks_per_file)
    }
}

/// Fetch a snapshot manifest from a URL.
pub async fn fetch_manifest(manifest_url: &str) -> Result<SnapshotManifest> {
    let client = Client::new();
    let manifest: SnapshotManifest =
        client.get(manifest_url).send().await?.error_for_status()?.json().await?;
    Ok(manifest)
}

/// Generate a manifest from local archive files on disk.
///
/// Scans the given directory for archive files matching the naming convention and computes
/// sizes and checksums.
pub fn generate_manifest(
    archive_dir: &Path,
    base_url: &str,
    block: u64,
    chain_id: u64,
    blocks_per_file: u64,
) -> Result<SnapshotManifest> {
    let mut components = BTreeMap::new();

    // State: single archive
    let state_path = archive_dir.join("state.tar.zst");
    if state_path.exists() {
        let size = std::fs::metadata(&state_path)?.len();
        components.insert(
            SnapshotComponentType::State.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size,
                checksum: None,
            }),
        );
        info!(target: "reth::cli", size = %super::DownloadProgress::format_size(size), "Found state archive");
    }

    // Indexes (rocksdb): single archive, optional
    let indexes_path = archive_dir.join("indexes.tar.zst");
    if indexes_path.exists() {
        let size = std::fs::metadata(&indexes_path)?.len();
        components.insert(
            SnapshotComponentType::Indexes.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "indexes.tar.zst".to_string(),
                size,
                checksum: None,
            }),
        );
        info!(target: "reth::cli", size = %super::DownloadProgress::format_size(size), "Found indexes archive");
    }

    // Chunked components — just record blocks_per_file + total_blocks,
    // the client derives chunk URLs from the naming convention.
    for ty in &[
        SnapshotComponentType::Headers,
        SnapshotComponentType::Transactions,
        SnapshotComponentType::Receipts,
        SnapshotComponentType::AccountChangesets,
        SnapshotComponentType::StorageChangesets,
    ] {
        let key = ty.key();
        // Check if at least the first chunk exists
        let first_end = blocks_per_file.min(block) - 1;
        let first_chunk = archive_dir.join(format!("{key}-0-{first_end}.tar.zst"));
        if first_chunk.exists() {
            let num_chunks = block.div_ceil(blocks_per_file);
            info!(target: "reth::cli",
                component = ty.display_name(),
                chunks = num_chunks,
                total_blocks = block,
                "Found chunked component"
            );
            components.insert(
                key.to_string(),
                ComponentManifest::Chunked(ChunkedArchive {
                    blocks_per_file,
                    total_blocks: block,
                }),
            );
        }
    }

    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();

    Ok(SnapshotManifest {
        block,
        chain_id,
        storage_version: "v2".to_string(),
        timestamp,
        base_url: base_url.to_string(),
        components,
    })
}

/// Resolves an archive file path from a component key and naming convention.
pub fn chunk_filename(component_key: &str, start: u64, end: u64) -> String {
    format!("{component_key}-{start}-{end}.tar.zst")
}
