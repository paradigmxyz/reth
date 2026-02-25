use blake3::Hasher;
use eyre::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path};
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
    /// Storage version (1 = legacy, 2 = current).
    pub storage_version: u64,
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
    /// Optional BLAKE3 checksum of the compressed archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
}

/// A chunked archive set where each chunk covers a fixed block range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkedArchive {
    /// Number of blocks per archive file. Matches reth's `blocks_per_file` config.
    pub blocks_per_file: u64,
    /// Total number of blocks covered by this component.
    pub total_blocks: u64,
    /// Compressed size of each chunk in bytes, ordered from first to last.
    /// Computed during manifest generation. Older manifests may omit this.
    #[serde(default)]
    pub chunk_sizes: Vec<u64>,
    /// BLAKE3 checksums per chunk, ordered from first to last.
    /// Computed during manifest generation. Older manifests may omit this.
    #[serde(default)]
    pub chunk_blake3: Vec<String>,
}

/// A single archive with concrete URL and optional integrity metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveDescriptor {
    pub url: String,
    pub file_name: String,
    pub size: u64,
    pub blake3: Option<String>,
}

/// How much of a component to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentSelection {
    /// Download all chunks (full archive).
    All,
    /// Download only the most recent chunks covering at least `distance` blocks.
    /// Maps to `PruneMode::Distance(distance)` in the generated config.
    Distance(u64),
    /// Don't download this component at all.
    /// Maps to `PruneMode::Full` for tx-based segments, or a minimal distance for others.
    None,
}

impl std::fmt::Display for ComponentSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "All"),
            Self::Distance(d) => write!(f, "Last {d} blocks"),
            Self::None => write!(f, "None"),
        }
    }
}

/// The types of snapshot components that can be downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SnapshotComponentType {
    /// State database (mdbx). Always required. Single archive.
    State,
    /// Block headers static files. Chunked.
    Headers,
    /// Transaction static files. Chunked.
    Transactions,
    /// Transaction sender static files. Chunked. Only downloaded for archive nodes.
    TransactionSenders,
    /// Receipt static files. Chunked.
    Receipts,
    /// Account changeset static files. Chunked.
    AccountChangesets,
    /// Storage changeset static files. Chunked.
    StorageChangesets,
    /// RocksDB index files. Single archive. Optional and archive-only.
    RocksdbIndices,
}

impl SnapshotComponentType {
    /// All component types in display order.
    pub const ALL: [Self; 8] = [
        Self::State,
        Self::Headers,
        Self::Transactions,
        Self::TransactionSenders,
        Self::Receipts,
        Self::AccountChangesets,
        Self::StorageChangesets,
        Self::RocksdbIndices,
    ];

    /// The string key used in the manifest JSON.
    pub const fn key(&self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Headers => "headers",
            Self::Transactions => "transactions",
            Self::TransactionSenders => "transaction_senders",
            Self::Receipts => "receipts",
            Self::AccountChangesets => "account_changesets",
            Self::StorageChangesets => "storage_changesets",
            Self::RocksdbIndices => "rocksdb_indices",
        }
    }

    /// Human-readable display name.
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::State => "State (mdbx)",
            Self::Headers => "Headers",
            Self::Transactions => "Transactions",
            Self::TransactionSenders => "Transaction Senders",
            Self::Receipts => "Receipts",
            Self::AccountChangesets => "Account Changesets",
            Self::StorageChangesets => "Storage Changesets",
            Self::RocksdbIndices => "RocksDB Indices",
        }
    }

    /// Whether this component is always required for a functional node.
    ///
    /// State and headers are always needed — a node cannot operate without block headers.
    pub const fn is_required(&self) -> bool {
        matches!(self, Self::State | Self::Headers)
    }

    /// Returns the default selection for this component in the minimal download preset.
    ///
    /// Matches the `--minimal` prune configuration:
    /// - State/Headers: always All (required)
    /// - Transactions/Changesets: Distance(10_064) (`MINIMUM_UNWIND_SAFE_DISTANCE`)
    /// - Receipts: Distance(64) (`MINIMUM_DISTANCE`)
    /// - TransactionSenders: None (only downloaded for archive nodes)
    /// - RocksdbIndices: None (only downloaded for archive nodes)
    ///
    /// `tx_lookup` and `sender_recovery` are always pruned full regardless.
    pub const fn minimal_selection(&self) -> ComponentSelection {
        match self {
            Self::State | Self::Headers => ComponentSelection::All,
            Self::Transactions | Self::AccountChangesets | Self::StorageChangesets => {
                ComponentSelection::Distance(10_064)
            }
            Self::Receipts => ComponentSelection::Distance(64),
            Self::TransactionSenders => ComponentSelection::None,
            Self::RocksdbIndices => ComponentSelection::None,
        }
    }

    /// Whether this component type uses chunked archives.
    pub const fn is_chunked(&self) -> bool {
        !matches!(self, Self::State)
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
                        let end = (i + 1) * chunked.blocks_per_file - 1;
                        format!("{}/{key}-{start}-{end}.tar.zst", self.base_url)
                    })
                    .collect()
            }
        }
    }

    /// Returns archive URLs for a component, limited to chunks covering at least `distance`
    /// blocks from the tip. Returns all URLs if distance is `None` (All mode).
    pub fn archive_urls_for_distance(
        &self,
        ty: SnapshotComponentType,
        distance: Option<u64>,
    ) -> Vec<String> {
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

                // Calculate which chunks to include
                let start_chunk = match distance {
                    Some(dist) => {
                        // We need chunks covering the last `dist` blocks
                        let needed_blocks = dist.min(chunked.total_blocks);
                        let needed_chunks = needed_blocks.div_ceil(chunked.blocks_per_file);
                        num_chunks.saturating_sub(needed_chunks)
                    }
                    None => 0, // All chunks
                };

                (start_chunk..num_chunks)
                    .map(|i| {
                        let start = i * chunked.blocks_per_file;
                        let end = (i + 1) * chunked.blocks_per_file - 1;
                        format!("{}/{key}-{start}-{end}.tar.zst", self.base_url)
                    })
                    .collect()
            }
        }
    }

    /// Returns concrete archive descriptors for a component, optionally limited to distance.
    pub fn archive_descriptors_for_distance(
        &self,
        ty: SnapshotComponentType,
        distance: Option<u64>,
    ) -> Vec<ArchiveDescriptor> {
        let Some(component) = self.component(ty) else {
            return vec![];
        };

        match component {
            ComponentManifest::Single(single) => {
                vec![ArchiveDescriptor {
                    url: format!("{}/{}", self.base_url, single.file),
                    file_name: single.file.clone(),
                    size: single.size,
                    blake3: single.blake3.clone(),
                }]
            }
            ComponentManifest::Chunked(chunked) => {
                let key = ty.key();
                let num_chunks = chunked.num_chunks();

                let start_chunk = match distance {
                    Some(dist) => {
                        let needed_blocks = dist.min(chunked.total_blocks);
                        let needed_chunks = needed_blocks.div_ceil(chunked.blocks_per_file);
                        num_chunks.saturating_sub(needed_chunks)
                    }
                    None => 0,
                };

                (start_chunk..num_chunks)
                    .map(|i| {
                        let start = i * chunked.blocks_per_file;
                        let end = (i + 1) * chunked.blocks_per_file - 1;
                        let file_name = format!("{key}-{start}-{end}.tar.zst");
                        let size = chunked.chunk_sizes.get(i as usize).copied().unwrap_or_default();
                        let blake3 = chunked
                            .chunk_blake3
                            .get(i as usize)
                            .cloned()
                            .filter(|hash| !hash.is_empty());

                        ArchiveDescriptor {
                            url: format!("{}/{}", self.base_url, file_name),
                            file_name,
                            size,
                            blake3,
                        }
                    })
                    .collect()
            }
        }
    }

    /// Returns the exact download size for a component given a distance selection.
    ///
    /// For single archives, returns the full size. For chunked archives, sums the
    /// sizes of the selected tail chunks from [`ChunkedArchive::chunk_sizes`].
    pub fn size_for_distance(&self, ty: SnapshotComponentType, distance: Option<u64>) -> u64 {
        let Some(component) = self.component(ty) else {
            return 0;
        };
        match component {
            ComponentManifest::Single(s) => s.size,
            ComponentManifest::Chunked(chunked) => {
                if chunked.chunk_sizes.is_empty() {
                    return 0;
                }
                let num_chunks = chunked.chunk_sizes.len() as u64;
                let start_chunk = match distance {
                    Some(dist) => {
                        let needed = dist.min(chunked.total_blocks);
                        let needed_chunks = needed.div_ceil(chunked.blocks_per_file);
                        num_chunks.saturating_sub(needed_chunks)
                    }
                    None => 0,
                };
                chunked.chunk_sizes[start_chunk as usize..].iter().sum()
            }
        }
    }

    /// Returns the number of chunks that would be downloaded for a given distance.
    pub fn chunks_for_distance(&self, ty: SnapshotComponentType, distance: Option<u64>) -> u64 {
        let Some(ComponentManifest::Chunked(chunked)) = self.component(ty) else {
            return if self.component(ty).is_some() { 1 } else { 0 };
        };
        match distance {
            Some(dist) => {
                let needed = dist.min(chunked.total_blocks);
                needed.div_ceil(chunked.blocks_per_file)
            }
            None => chunked.num_chunks(),
        }
    }
}

impl ComponentManifest {
    /// Returns the total download size for this component.
    pub fn total_size(&self) -> u64 {
        match self {
            Self::Single(s) => s.size,
            Self::Chunked(c) => c.chunk_sizes.iter().sum(),
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
        let blake3 = Some(file_blake3_hex(&state_path)?);
        components.insert(
            SnapshotComponentType::State.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size,
                blake3,
            }),
        );
        info!(target: "reth::cli", size = %super::DownloadProgress::format_size(size), "Found state archive");
    }

    // Optional archive-only RocksDB indices: single archive
    let rocksdb_indices_path = archive_dir.join("rocksdb_indices.tar.zst");
    if rocksdb_indices_path.exists() {
        let size = std::fs::metadata(&rocksdb_indices_path)?.len();
        let blake3 = Some(file_blake3_hex(&rocksdb_indices_path)?);
        components.insert(
            SnapshotComponentType::RocksdbIndices.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "rocksdb_indices.tar.zst".to_string(),
                size,
                blake3,
            }),
        );
        info!(target: "reth::cli", size = %super::DownloadProgress::format_size(size), "Found RocksDB indices archive");
    }

    // Chunked components — record blocks_per_file, total_blocks, and sum chunk sizes.
    for ty in &[
        SnapshotComponentType::Headers,
        SnapshotComponentType::Transactions,
        SnapshotComponentType::TransactionSenders,
        SnapshotComponentType::Receipts,
        SnapshotComponentType::AccountChangesets,
        SnapshotComponentType::StorageChangesets,
    ] {
        let key = ty.key();
        let first_end = blocks_per_file - 1;
        let first_chunk = archive_dir.join(format!("{key}-0-{first_end}.tar.zst"));
        if first_chunk.exists() {
            let num_chunks = block.div_ceil(blocks_per_file);

            // Collect per-chunk sizes
            let mut chunk_sizes = Vec::with_capacity(num_chunks as usize);
            let mut chunk_blake3 = Vec::with_capacity(num_chunks as usize);
            for i in 0..num_chunks {
                let start = i * blocks_per_file;
                let end = (i + 1) * blocks_per_file - 1;
                let chunk_path = archive_dir.join(format!("{key}-{start}-{end}.tar.zst"));
                let size = std::fs::metadata(&chunk_path).map(|m| m.len()).unwrap_or(0);
                chunk_sizes.push(size);
                let checksum =
                    if chunk_path.exists() { file_blake3_hex(&chunk_path)? } else { String::new() };
                chunk_blake3.push(checksum);
            }

            let total_size: u64 = chunk_sizes.iter().sum();
            info!(target: "reth::cli",
                component = ty.display_name(),
                chunks = num_chunks,
                total_blocks = block,
                size = %super::DownloadProgress::format_size(total_size),
                "Found chunked component"
            );
            components.insert(
                key.to_string(),
                ComponentManifest::Chunked(ChunkedArchive {
                    blocks_per_file,
                    total_blocks: block,
                    chunk_sizes,
                    chunk_blake3,
                }),
            );
        }
    }

    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();

    Ok(SnapshotManifest {
        block,
        chain_id,
        storage_version: 2,
        timestamp,
        base_url: base_url.to_string(),
        components,
    })
}

/// Resolves an archive file path from a component key and naming convention.
pub fn chunk_filename(component_key: &str, start: u64, end: u64) -> String {
    format!("{component_key}-{start}-{end}.tar.zst")
}

fn file_blake3_hex(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buf = [0_u8; 64 * 1024];

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manifest() -> SnapshotManifest {
        let mut components = BTreeMap::new();
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size: 100,
                blake3: None,
            }),
        );
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_500_000,
                chunk_sizes: vec![80_000, 100_000, 120_000],
                chunk_blake3: vec![],
            }),
        );
        components.insert(
            "headers".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_500_000,
                chunk_sizes: vec![40_000, 50_000, 60_000],
                chunk_blake3: vec![],
            }),
        );
        SnapshotManifest {
            block: 1_500_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: "https://example.com".to_string(),
            components,
        }
    }

    #[test]
    fn archive_urls_for_distance_all() {
        let m = test_manifest();
        let urls = m.archive_urls_for_distance(SnapshotComponentType::Transactions, None);
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://example.com/transactions-0-499999.tar.zst");
        assert_eq!(urls[2], "https://example.com/transactions-1000000-1499999.tar.zst");
    }

    #[test]
    fn archive_urls_for_distance_partial() {
        let m = test_manifest();
        // 600k blocks → needs 2 chunks (each 500k)
        let urls = m.archive_urls_for_distance(SnapshotComponentType::Transactions, Some(600_000));
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com/transactions-500000-999999.tar.zst");
        assert_eq!(urls[1], "https://example.com/transactions-1000000-1499999.tar.zst");
    }

    #[test]
    fn archive_urls_for_distance_single_component() {
        let m = test_manifest();
        // Single archives always return one URL regardless of distance
        let urls = m.archive_urls_for_distance(SnapshotComponentType::State, Some(100));
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com/state.tar.zst");
    }

    #[test]
    fn archive_urls_for_distance_rocksdb_indices_single_component() {
        let mut components = BTreeMap::new();
        components.insert(
            "rocksdb_indices".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "rocksdb_indices.tar.zst".to_string(),
                size: 777,
                blake3: None,
            }),
        );
        let m = SnapshotManifest {
            block: 1,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: "https://example.com".to_string(),
            components,
        };

        let urls = m.archive_urls_for_distance(SnapshotComponentType::RocksdbIndices, Some(10));
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com/rocksdb_indices.tar.zst");
        assert_eq!(m.size_for_distance(SnapshotComponentType::RocksdbIndices, Some(10)), 777);
    }

    #[test]
    fn archive_urls_for_distance_missing_component() {
        let m = test_manifest();
        let urls = m.archive_urls_for_distance(SnapshotComponentType::Receipts, None);
        assert!(urls.is_empty());
    }

    #[test]
    fn chunks_for_distance_all() {
        let m = test_manifest();
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::Transactions, None), 3);
    }

    #[test]
    fn chunks_for_distance_partial() {
        let m = test_manifest();
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::Transactions, Some(600_000)), 2);
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::Transactions, Some(100_000)), 1);
    }

    #[test]
    fn chunks_for_distance_single() {
        let m = test_manifest();
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::State, None), 1);
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::State, Some(100)), 1);
    }

    #[test]
    fn chunks_for_distance_missing() {
        let m = test_manifest();
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::Receipts, None), 0);
    }

    #[test]
    fn component_selection_display() {
        assert_eq!(ComponentSelection::All.to_string(), "All");
        assert_eq!(ComponentSelection::Distance(10_064).to_string(), "Last 10064 blocks");
        assert_eq!(ComponentSelection::None.to_string(), "None");
    }

    #[test]
    fn archive_urls_aligned_to_blocks_per_file() {
        // When total_blocks is not aligned to blocks_per_file, chunk boundaries
        // must still align to blocks_per_file (not total_blocks).
        let mut components = BTreeMap::new();
        components.insert(
            "storage_changesets".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 24_396_822,
                chunk_sizes: vec![100; 49], // 49 chunks
                chunk_blake3: vec![],
            }),
        );
        let m = SnapshotManifest {
            block: 24_396_822,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: "https://example.com".to_string(),
            components,
        };
        let urls = m.archive_urls(SnapshotComponentType::StorageChangesets);
        assert_eq!(urls.len(), 49);
        // First chunk: 0-499999 (not 0-396821 or similar)
        assert_eq!(urls[0], "https://example.com/storage_changesets-0-499999.tar.zst");
        // Last chunk: 24000000-24499999 (not 24000000-24396821)
        assert_eq!(urls[48], "https://example.com/storage_changesets-24000000-24499999.tar.zst");
    }

    #[test]
    fn size_for_distance_sums_tail_chunks() {
        let m = test_manifest();
        // Transactions has chunk_sizes [80_000, 100_000, 120_000]
        // All: sum of all 3
        assert_eq!(m.size_for_distance(SnapshotComponentType::Transactions, None), 300_000);
        // Last 500K blocks = 1 chunk = last chunk only
        assert_eq!(
            m.size_for_distance(SnapshotComponentType::Transactions, Some(500_000)),
            120_000
        );
        // Last 600K blocks = 2 chunks = last two
        assert_eq!(
            m.size_for_distance(SnapshotComponentType::Transactions, Some(600_000)),
            220_000
        );
        // Single archive (state) always returns full size
        assert_eq!(m.size_for_distance(SnapshotComponentType::State, Some(100)), 100);
        // Missing component
        assert_eq!(m.size_for_distance(SnapshotComponentType::Receipts, None), 0);
    }

    #[test]
    fn archive_descriptors_include_checksum_metadata() {
        let mut components = BTreeMap::new();
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size: 100,
                blake3: Some("abc123".to_string()),
            }),
        );
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_000_000,
                chunk_sizes: vec![80_000, 120_000],
                chunk_blake3: vec!["hash0".to_string(), "hash1".to_string()],
            }),
        );

        let m = SnapshotManifest {
            block: 1_000_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: "https://example.com".to_string(),
            components,
        };

        let state = m.archive_descriptors_for_distance(SnapshotComponentType::State, None);
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].file_name, "state.tar.zst");
        assert_eq!(state[0].blake3.as_deref(), Some("abc123"));

        let tx = m.archive_descriptors_for_distance(SnapshotComponentType::Transactions, None);
        assert_eq!(tx.len(), 2);
        assert_eq!(tx[0].blake3.as_deref(), Some("hash0"));
        assert_eq!(tx[1].blake3.as_deref(), Some("hash1"));
    }
}
