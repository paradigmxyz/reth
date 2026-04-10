use blake3::Hasher;
use eyre::Result;
use rayon::prelude::*;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};
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
    ///
    /// When omitted, downloaders should derive the base URL from the manifest URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Reth version that produced this snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reth_version: Option<String>,
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
    /// Expected extracted plain files for this archive.
    ///
    /// This is the authoritative integrity source for the modular download path.
    #[serde(default)]
    pub output_files: Vec<OutputFileChecksum>,
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
    /// Expected extracted plain files per chunk, ordered from first to last.
    ///
    /// This is the authoritative integrity source for the modular download path.
    #[serde(default)]
    pub chunk_output_files: Vec<Vec<OutputFileChecksum>>,
}

/// Expected metadata for one extracted plain file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputFileChecksum {
    /// Relative path under the target datadir where this file is extracted.
    pub path: String,
    /// Plain file size in bytes.
    pub size: u64,
    /// BLAKE3 checksum of the plain file contents.
    pub blake3: String,
}

/// A single archive with concrete URL and optional integrity metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveDescriptor {
    pub url: String,
    pub file_name: String,
    pub size: u64,
    pub blake3: Option<String>,
    pub output_files: Vec<OutputFileChecksum>,
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
        !matches!(self, Self::State | Self::RocksdbIndices)
    }
}

impl SnapshotManifest {
    fn base_url_or_empty(&self) -> &str {
        self.base_url.as_deref().unwrap_or("")
    }

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
                vec![format!("{}/{}", self.base_url_or_empty(), single.file)]
            }
            ComponentManifest::Chunked(chunked) => {
                let key = ty.key();
                let num_chunks = chunked.num_chunks();
                (0..num_chunks)
                    .map(|i| {
                        let start = i * chunked.blocks_per_file;
                        let end = (i + 1) * chunked.blocks_per_file - 1;
                        format!("{}/{key}-{start}-{end}.tar.zst", self.base_url_or_empty())
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
                vec![format!("{}/{}", self.base_url_or_empty(), single.file)]
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
                        format!("{}/{key}-{start}-{end}.tar.zst", self.base_url_or_empty())
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
                    url: format!("{}/{}", self.base_url_or_empty(), single.file),
                    file_name: single.file.clone(),
                    size: single.size,
                    blake3: single.blake3.clone(),
                    output_files: single.output_files.clone(),
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
                        let output_files =
                            chunked.chunk_output_files.get(i as usize).cloned().unwrap_or_default();

                        ArchiveDescriptor {
                            url: format!("{}/{}", self.base_url_or_empty(), file_name),
                            file_name,
                            size,
                            blake3: None,
                            output_files,
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

/// Package chunk archives from a source datadir and generate a manifest.
pub fn generate_manifest(
    source_datadir: &Path,
    output_dir: &Path,
    base_url: Option<&str>,
    block: u64,
    chain_id: u64,
    blocks_per_file: u64,
) -> Result<SnapshotManifest> {
    std::fs::create_dir_all(output_dir)?;

    let mut components = BTreeMap::new();

    // Package chunked static-file components.
    for ty in &[
        SnapshotComponentType::Headers,
        SnapshotComponentType::Transactions,
        SnapshotComponentType::TransactionSenders,
        SnapshotComponentType::Receipts,
        SnapshotComponentType::AccountChangesets,
        SnapshotComponentType::StorageChangesets,
    ] {
        let key = ty.key();
        let num_chunks = block.div_ceil(blocks_per_file);
        let mut planned_chunks = Vec::with_capacity(num_chunks as usize);
        let mut found_any = false;

        for i in 0..num_chunks {
            let start = i * blocks_per_file;
            let end = (i + 1) * blocks_per_file - 1;
            let source_files = source_files_for_chunk(source_datadir, *ty, start, end)?;

            if source_files.is_empty() {
                if found_any {
                    eyre::bail!("Missing source files for {} chunk {}-{}", key, start, end);
                }
                continue;
            }

            found_any = true;
            planned_chunks.push(PlannedChunk {
                chunk_idx: i,
                archive_path: output_dir.join(chunk_filename(key, start, end)),
                source_files,
            });
        }

        if found_any {
            let mut packaged_chunks = planned_chunks
                .into_par_iter()
                .map(|planned| -> Result<PackagedChunk> {
                    let output_files =
                        write_chunk_archive(&planned.archive_path, &planned.source_files)?;
                    let size = std::fs::metadata(&planned.archive_path)?.len();
                    Ok(PackagedChunk { chunk_idx: planned.chunk_idx, size, output_files })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .collect::<Result<Vec<_>>>()?;

            packaged_chunks.sort_unstable_by_key(|chunk| chunk.chunk_idx);
            let chunk_sizes = packaged_chunks.iter().map(|chunk| chunk.size).collect::<Vec<_>>();
            let chunk_output_files =
                packaged_chunks.into_iter().map(|chunk| chunk.output_files).collect::<Vec<_>>();
            let total_size: u64 = chunk_sizes.iter().sum();
            info!(target: "reth::cli",
                component = ty.display_name(),
                chunks = chunk_sizes.len(),
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
                    chunk_output_files,
                }),
            );
        }
    }

    let (state_size, state_output_files) = package_single_component(
        output_dir,
        "state.tar.zst",
        &state_source_files(source_datadir)?,
    )?;
    components.insert(
        SnapshotComponentType::State.key().to_string(),
        ComponentManifest::Single(SingleArchive {
            file: "state.tar.zst".to_string(),
            size: state_size,
            blake3: None,
            output_files: state_output_files,
        }),
    );

    let rocksdb_files = rocksdb_source_files(source_datadir)?;
    if !rocksdb_files.is_empty() {
        let (rocksdb_size, rocksdb_output_files) =
            package_single_component(output_dir, "rocksdb_indices.tar.zst", &rocksdb_files)?;
        components.insert(
            SnapshotComponentType::RocksdbIndices.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "rocksdb_indices.tar.zst".to_string(),
                size: rocksdb_size,
                blake3: None,
                output_files: rocksdb_output_files,
            }),
        );
    }

    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();

    Ok(SnapshotManifest {
        block,
        chain_id,
        storage_version: 2,
        timestamp,
        base_url: base_url.map(str::to_owned),
        reth_version: Some(reth_node_core::version::version_metadata().short_version.to_string()),
        components,
    })
}

/// Resolves an archive file path from a component key and naming convention.
pub fn chunk_filename(component_key: &str, start: u64, end: u64) -> String {
    format!("{component_key}-{start}-{end}.tar.zst")
}

#[derive(Debug)]
struct PlannedChunk {
    chunk_idx: u64,
    archive_path: PathBuf,
    source_files: Vec<PathBuf>,
}

#[derive(Debug)]
struct PackagedChunk {
    chunk_idx: u64,
    size: u64,
    output_files: Vec<OutputFileChecksum>,
}

#[derive(Debug)]
struct PlannedFile {
    source_path: PathBuf,
    relative_path: PathBuf,
}

fn source_files_for_chunk(
    source_datadir: &Path,
    component: SnapshotComponentType,
    start: u64,
    end: u64,
) -> Result<Vec<PathBuf>> {
    let Some(segment_name) = static_segment_name(component) else {
        return Ok(Vec::new());
    };

    let static_files_dir = source_datadir.join("static_files");
    let static_files_dir =
        if static_files_dir.exists() { static_files_dir } else { source_datadir.to_path_buf() };
    let prefix = format!("static_file_{segment_name}_{start}_{end}");

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&static_files_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            files.push(entry.path());
        }
    }

    files.sort_unstable();
    Ok(files)
}

fn static_segment_name(component: SnapshotComponentType) -> Option<&'static str> {
    match component {
        SnapshotComponentType::Headers => Some("headers"),
        SnapshotComponentType::Transactions => Some("transactions"),
        SnapshotComponentType::TransactionSenders => Some("transaction-senders"),
        SnapshotComponentType::Receipts => Some("receipts"),
        SnapshotComponentType::AccountChangesets => Some("account-change-sets"),
        SnapshotComponentType::StorageChangesets => Some("storage-change-sets"),
        SnapshotComponentType::State | SnapshotComponentType::RocksdbIndices => None,
    }
}

fn state_source_files(source_datadir: &Path) -> Result<Vec<PlannedFile>> {
    let db_dir = source_datadir.join("db");
    if db_dir.exists() {
        return collect_files_recursive(&db_dir, Path::new("db"));
    }

    if looks_like_db_dir(source_datadir)? {
        return collect_files_recursive(source_datadir, Path::new("db"));
    }

    eyre::bail!("Could not find source state DB directory under {}", source_datadir.display())
}

fn rocksdb_source_files(source_datadir: &Path) -> Result<Vec<PlannedFile>> {
    let rocksdb_dir = source_datadir.join("rocksdb");
    if !rocksdb_dir.exists() {
        return Ok(Vec::new());
    }

    collect_files_recursive(&rocksdb_dir, Path::new("rocksdb"))
}

fn looks_like_db_dir(path: &Path) -> Result<bool> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return Ok(false),
    };

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "mdbx.dat" || name == "lock.mdb" || name == "data.mdb" {
            return Ok(true);
        }
    }

    Ok(false)
}

fn collect_files_recursive(root: &Path, output_prefix: &Path) -> Result<Vec<PlannedFile>> {
    let mut files = Vec::new();
    collect_files_recursive_inner(root, root, output_prefix, &mut files)?;
    files.sort_unstable_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

fn collect_files_recursive_inner(
    root: &Path,
    dir: &Path,
    output_prefix: &Path,
    files: &mut Vec<PlannedFile>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_recursive_inner(root, &path, output_prefix, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let relative = path.strip_prefix(root)?.to_path_buf();
        files.push(PlannedFile { source_path: path, relative_path: output_prefix.join(relative) });
    }

    Ok(())
}

fn package_single_component(
    output_dir: &Path,
    archive_file_name: &str,
    files: &[PlannedFile],
) -> Result<(u64, Vec<OutputFileChecksum>)> {
    if files.is_empty() {
        eyre::bail!("Cannot package empty single archive: {}", archive_file_name);
    }

    let archive_path = output_dir.join(archive_file_name);
    let output_files = write_archive_from_planned_files(&archive_path, files)?;
    let size = std::fs::metadata(&archive_path)?.len();
    Ok((size, output_files))
}

fn write_chunk_archive(path: &Path, source_files: &[PathBuf]) -> Result<Vec<OutputFileChecksum>> {
    let planned_files = source_files
        .iter()
        .map(|source_path| {
            let file_name = source_path.file_name().ok_or_else(|| {
                eyre::eyre!("Invalid source file path: {}", source_path.display())
            })?;
            Ok::<_, eyre::Error>(PlannedFile {
                source_path: source_path.clone(),
                relative_path: PathBuf::from("static_files").join(file_name),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    write_archive_from_planned_files(path, &planned_files)
}

fn write_archive_from_planned_files(
    path: &Path,
    files: &[PlannedFile],
) -> Result<Vec<OutputFileChecksum>> {
    let file = std::fs::File::create(path)?;
    let mut encoder = zstd::Encoder::new(file, 0)?;
    // Emit standard zstd frames with checksums for compatibility with external
    // tools such as `pzstd -d`.
    encoder.include_checksum(true)?;
    let mut builder = tar::Builder::new(encoder);

    let mut output_files = Vec::with_capacity(files.len());
    for planned in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(std::fs::metadata(&planned.source_path)?.len());
        header.set_mode(0o644);
        header.set_cksum();

        let source_file = std::fs::File::open(&planned.source_path)?;
        let mut reader = HashingReader::new(source_file);
        builder.append_data(&mut header, &planned.relative_path, &mut reader)?;

        output_files.push(OutputFileChecksum {
            path: planned.relative_path.to_string_lossy().to_string(),
            size: reader.bytes_read,
            blake3: reader.finalize(),
        });
    }

    builder.finish()?;
    let encoder = builder.into_inner()?;
    encoder.finish()?;

    Ok(output_files)
}

struct HashingReader<R> {
    inner: R,
    hasher: Hasher,
    bytes_read: u64,
}

impl<R: Read> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self { inner, hasher: Hasher::new(), bytes_read: 0 }
    }

    fn finalize(self) -> String {
        self.hasher.finalize().to_hex().to_string()
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.bytes_read += n as u64;
            self.hasher.update(&buf[..n]);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_manifest() -> SnapshotManifest {
        let mut components = BTreeMap::new();
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size: 100,
                blake3: None,
                output_files: vec![],
            }),
        );
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_500_000,
                chunk_sizes: vec![80_000, 100_000, 120_000],
                chunk_output_files: vec![vec![], vec![], vec![]],
            }),
        );
        components.insert(
            "headers".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_500_000,
                chunk_sizes: vec![40_000, 50_000, 60_000],
                chunk_output_files: vec![vec![], vec![], vec![]],
            }),
        );
        SnapshotManifest {
            block: 1_500_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
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
                output_files: vec![],
            }),
        );
        let m = SnapshotManifest {
            block: 1,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
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
                chunk_output_files: vec![vec![]; 49],
            }),
        );
        let m = SnapshotManifest {
            block: 24_396_822,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
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
                output_files: vec![OutputFileChecksum {
                    path: "db/mdbx.dat".to_string(),
                    size: 1000,
                    blake3: "s0".to_string(),
                }],
            }),
        );
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_000_000,
                chunk_sizes: vec![80_000, 120_000],
                chunk_output_files: vec![
                    vec![OutputFileChecksum {
                        path: "static_files/static_file_transactions_0_499999.bin".to_string(),
                        size: 111,
                        blake3: "h0".to_string(),
                    }],
                    vec![OutputFileChecksum {
                        path: "static_files/static_file_transactions_500000_999999.bin".to_string(),
                        size: 222,
                        blake3: "h1".to_string(),
                    }],
                ],
            }),
        );

        let m = SnapshotManifest {
            block: 1_000_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
            components,
        };

        let state = m.archive_descriptors_for_distance(SnapshotComponentType::State, None);
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].file_name, "state.tar.zst");
        assert_eq!(state[0].blake3.as_deref(), Some("abc123"));
        assert_eq!(state[0].output_files.len(), 1);

        let tx = m.archive_descriptors_for_distance(SnapshotComponentType::Transactions, None);
        assert_eq!(tx.len(), 2);
        assert_eq!(tx[0].blake3, None);
        assert_eq!(tx[1].blake3, None);
        assert_eq!(tx[0].output_files[0].size, 111);
    }

    #[test]
    fn generate_manifest_includes_state_single_archive() {
        let source = tempdir().unwrap();
        let output = tempdir().unwrap();
        let db_dir = source.path().join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::write(db_dir.join("mdbx.dat"), b"state-data").unwrap();

        let manifest =
            generate_manifest(source.path(), output.path(), None, 0, 1, 500_000).unwrap();

        let state = manifest.component(SnapshotComponentType::State).unwrap();
        let ComponentManifest::Single(state) = state else {
            panic!("state should be a single archive")
        };
        assert_eq!(state.file, "state.tar.zst");
        assert!(!state.output_files.is_empty());
        assert_eq!(state.output_files[0].path, "db/mdbx.dat");
        assert!(output.path().join("state.tar.zst").exists());
    }

    #[test]
    fn generate_manifest_includes_rocksdb_single_archive_when_present() {
        let source = tempdir().unwrap();
        let output = tempdir().unwrap();
        let db_dir = source.path().join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::write(db_dir.join("mdbx.dat"), b"state-data").unwrap();
        let rocksdb_dir = source.path().join("rocksdb");
        std::fs::create_dir_all(&rocksdb_dir).unwrap();
        std::fs::write(rocksdb_dir.join("CURRENT"), b"MANIFEST-000001").unwrap();

        let manifest =
            generate_manifest(source.path(), output.path(), None, 0, 1, 500_000).unwrap();

        let rocksdb = manifest.component(SnapshotComponentType::RocksdbIndices).unwrap();
        let ComponentManifest::Single(rocksdb) = rocksdb else {
            panic!("rocksdb indices should be a single archive")
        };
        assert_eq!(rocksdb.file, "rocksdb_indices.tar.zst");
        assert!(!rocksdb.output_files.is_empty());
        assert_eq!(rocksdb.output_files[0].path, "rocksdb/CURRENT");
        assert!(output.path().join("rocksdb_indices.tar.zst").exists());
    }
}
