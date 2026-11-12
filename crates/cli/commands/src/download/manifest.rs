use blake3::Hasher;
use eyre::{Result, WrapErr};
use rayon::prelude::*;
use reqwest::Client;
use reth_db::static_file::iter_static_files;
use reth_static_file_types::StaticFileSegment;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Component, Path, PathBuf},
};
use tracing::info;
use url::Url;

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// A snapshot manifest describes available components for a snapshot at a given block height.
///
/// Each component is either a single archive (state), a set of chunked archives (static file
/// segments like transactions, receipts, etc), or a set of per-file archives (RocksDB indices).
/// Chunked components use `blocks_per_file` to define the block range per archive, matching
/// reth's static file segment boundaries.
///
/// Archive paths are resolved relative to [`SnapshotManifest::base_url`] via URL joining.
/// Single archives use [`SingleArchive::file`]; chunked archives default to
/// `{component}-{start_block}-{end_block}.tar.zst`, or use [`ChunkedArchive::chunk_files`]
/// when present so publishers can place finalized and tip chunks under different prefixes, or
/// reference an earlier snapshot's archives by absolute URL:
///
/// ```text
/// base_url = https://example.com/mainnet
///   static_files/transactions-0-499999.tar.zst
///   1700000/transactions-500000-999999.tar.zst
///   1700000/state.tar.zst
/// ```
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
    /// Chain-specific manifest fields not interpreted by Reth.
    ///
    /// Extensions are retained so downstream commands can consume snapshot metadata without
    /// refetching or reparsing the manifest selected by Reth.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

/// Manifest entry for a single snapshot component.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComponentManifest {
    /// A single archive file (used for state).
    Single(SingleArchive),
    /// A set of chunked archives split by block range (used for static file segments).
    Chunked(ChunkedArchive),
    /// A set of independent archives (used for RocksDB indices).
    Files(ArchiveSet),
}

/// A single, non-chunked archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleArchive {
    /// Archive file name (relative to base_url).
    ///
    /// Must be a relative path (no leading `/`). Nested paths like `1700000/state.tar.zst`
    /// are supported when `base_url` points at the snapshot root.
    pub file: String,
    /// Compressed archive size in bytes.
    pub size: u64,
    /// Total extracted plain-output size in bytes.
    ///
    /// Older manifests may omit this, in which case downloaders should derive it from
    /// `output_files`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub decompressed_size: u64,
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
    /// Total number of blocks covered by this component, including block zero.
    ///
    /// Coverage is contiguous from block zero. The final chunk may be partial; its actual
    /// last block is `total_blocks - 1`, irrespective of its filename's range end.
    /// Older manifests may store the declared snapshot block instead of the count.
    pub total_blocks: u64,
    /// Compressed size of each chunk in bytes, ordered from first to last.
    /// Computed during manifest generation. Older manifests may omit this.
    #[serde(default)]
    pub chunk_sizes: Vec<u64>,
    /// Extracted plain-output size of each chunk in bytes, ordered from first to last.
    ///
    /// Older manifests may omit this, in which case downloaders should derive it from
    /// `chunk_output_files`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunk_decompressed_sizes: Vec<u64>,
    /// Archive location for each chunk, ordered from first to last.
    ///
    /// When empty (older manifests), downloaders fall back to the default
    /// `{component}-{start}-{end}.tar.zst` name. When set, the length must equal the chunk
    /// count. Relative paths are joined with [`SnapshotManifest::base_url`] so publishers can
    /// place chunks under different prefixes (for example `static_files/…` for finalized chunks
    /// and `{timestamp}/…` for the tip chunk). Absolute URLs, such as archives reused from a
    /// previous snapshot, are used as-is.
    ///
    /// Relative paths must not start with `/`: URL joining treats a leading slash as
    /// host-absolute and would drop the `base_url` path prefix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunk_files: Vec<String>,
    /// Expected extracted plain files per chunk, ordered from first to last.
    ///
    /// This is the authoritative integrity source for the modular download path.
    #[serde(default)]
    pub chunk_output_files: Vec<Vec<OutputFileChecksum>>,
}

/// A set of archives that are downloaded, verified, and reused independently.
///
/// RocksDB SST files are immutable once written, and consecutive snapshots of the same database
/// share almost all of them. Packaging each SST in its own archive, named after the file and its
/// BLAKE3 checksum, lets publishers upload an archive once and reference it from later
/// manifests, and lets downloads resume per file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveSet {
    /// Archives in this set, each with its own path relative to [`SnapshotManifest::base_url`].
    pub archives: Vec<SingleArchive>,
}

/// Expected metadata for one extracted plain file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputFileChecksum {
    /// Relative path under the target datadir where this file is extracted.
    pub path: String,
    /// Plain file size in bytes.
    pub size: u64,
    /// BLAKE3 checksum of the plain file contents.
    pub blake3: String,
}

impl OutputFileChecksum {
    /// Returns the path as a relative path that stays below the directory it is joined to.
    ///
    /// Only normal components are accepted, plus the `./` prefix tar adds to entry paths.
    /// Absolute paths and `..` are rejected because the path is joined onto the datadir for
    /// verification and for removal before retries.
    pub(crate) fn relative_path(&self) -> Result<&Path> {
        let path = Path::new(&self.path);
        eyre::ensure!(
            path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir)) &&
                path.components().any(|part| matches!(part, Component::Normal(_))),
            "Invalid manifest output file path `{}`: must be a relative path without `..`",
            self.path
        );
        Ok(path)
    }
}

/// A concrete snapshot archive with its download and verification metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotArchive {
    pub url: String,
    pub file_name: String,
    pub size: u64,
    pub blake3: Option<String>,
    pub output_files: Vec<OutputFileChecksum>,
}

impl SnapshotArchive {
    /// Returns the total extracted plain-output size for this archive.
    pub fn output_size(&self) -> u64 {
        self.output_files.iter().map(|file| file.size).sum()
    }
}

/// How much of a component to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentSelection {
    /// Download all chunks (full archive).
    All,
    /// Download only the most recent chunks covering at least `distance` blocks.
    /// Maps to `PruneMode::Distance(distance)` in the generated config.
    Distance(u64),
    /// Download chunks starting at the specified block number.
    /// Maps to `PruneMode::Before(block)` in the generated config.
    Since(u64),
    /// Don't download this component at all.
    /// Maps to `PruneMode::Full` for tx-based segments, or a minimal distance for others.
    None,
}

impl std::fmt::Display for ComponentSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "All"),
            Self::Distance(d) => write!(f, "Last {d} blocks"),
            Self::Since(block) => write!(f, "Since block {block}"),
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
    /// RocksDB index files. One archive per SST file plus one for the remaining files.
    /// Optional and archive-only.
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
                vec![resolve_archive_url(self.base_url_or_empty(), &single.file)]
            }
            ComponentManifest::Files(set) => set
                .archives
                .iter()
                .map(|archive| resolve_archive_url(self.base_url_or_empty(), &archive.file))
                .collect(),
            ComponentManifest::Chunked(chunked) => {
                let key = ty.key();
                let num_chunks = chunked.num_chunks();
                (0..num_chunks)
                    .map(|i| {
                        resolve_archive_url(
                            self.base_url_or_empty(),
                            &chunked.chunk_relative_path(key, i),
                        )
                    })
                    .collect()
            }
        }
    }

    /// Returns archive URLs for a component, limited to chunks covering at least `distance`
    /// blocks ending at [`Self::block`]. Returns all URLs if distance is `None` (All mode).
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
                vec![resolve_archive_url(self.base_url_or_empty(), &single.file)]
            }
            ComponentManifest::Files(set) => set
                .archives
                .iter()
                .map(|archive| resolve_archive_url(self.base_url_or_empty(), &archive.file))
                .collect(),
            ComponentManifest::Chunked(chunked) => {
                let key = ty.key();
                let num_chunks = chunked.num_chunks();

                let start_chunk = self.start_chunk_for_distance(chunked, distance);

                (start_chunk..num_chunks)
                    .map(|i| {
                        resolve_archive_url(
                            self.base_url_or_empty(),
                            &chunked.chunk_relative_path(key, i),
                        )
                    })
                    .collect()
            }
        }
    }

    /// Returns concrete snapshot archives for a component, optionally limited to a distance
    /// ending at [`Self::block`].
    pub fn snapshot_archives_for_distance(
        &self,
        ty: SnapshotComponentType,
        distance: Option<u64>,
    ) -> Vec<SnapshotArchive> {
        let Some(component) = self.component(ty) else {
            return vec![];
        };

        match component {
            ComponentManifest::Single(single) => {
                vec![single.snapshot_archive(self.base_url_or_empty())]
            }
            ComponentManifest::Files(set) => set
                .archives
                .iter()
                .map(|archive| archive.snapshot_archive(self.base_url_or_empty()))
                .collect(),
            ComponentManifest::Chunked(chunked) => {
                let key = ty.key();
                let num_chunks = chunked.num_chunks();

                let start_chunk = self.start_chunk_for_distance(chunked, distance);

                (start_chunk..num_chunks)
                    .map(|i| {
                        let file_name = chunked.chunk_relative_path(key, i);
                        let size = chunked.chunk_sizes.get(i as usize).copied().unwrap_or_default();
                        let output_files =
                            chunked.chunk_output_files.get(i as usize).cloned().unwrap_or_default();

                        SnapshotArchive {
                            url: resolve_archive_url(self.base_url_or_empty(), &file_name),
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

    /// Fails if any output file path in the manifest is not a plain relative path.
    ///
    /// Output paths come from an untrusted manifest, so this is checked before any archive is
    /// planned.
    pub fn validate_output_paths(&self) -> Result<()> {
        for (key, component) in &self.components {
            let outputs: Box<dyn Iterator<Item = &OutputFileChecksum>> = match component {
                ComponentManifest::Single(single) => Box::new(single.output_files.iter()),
                ComponentManifest::Chunked(chunked) => {
                    Box::new(chunked.chunk_output_files.iter().flatten())
                }
                ComponentManifest::Files(set) => {
                    Box::new(set.archives.iter().flat_map(|archive| &archive.output_files))
                }
            };
            for output in outputs {
                output.relative_path().wrap_err_with(|| format!("In component `{key}`"))?;
            }
        }
        Ok(())
    }

    /// Returns the exact download size for a component given a distance ending at [`Self::block`].
    ///
    /// For single archives and archive sets, returns the full size. For chunked archives, sums
    /// the sizes of the selected tail chunks from [`ChunkedArchive::chunk_sizes`].
    pub fn size_for_distance(&self, ty: SnapshotComponentType, distance: Option<u64>) -> u64 {
        let Some(component) = self.component(ty) else {
            return 0;
        };
        match component {
            ComponentManifest::Single(_) | ComponentManifest::Files(_) => component.total_size(),
            ComponentManifest::Chunked(chunked) => {
                if chunked.chunk_sizes.is_empty() {
                    return 0;
                }
                let start_chunk = self.start_chunk_for_distance(chunked, distance);
                chunked.chunk_sizes.iter().skip(start_chunk as usize).sum()
            }
        }
    }

    /// Returns the exact extracted plain-output size for a component given a distance ending at
    /// [`Self::block`].
    pub fn output_size_for_distance(
        &self,
        ty: SnapshotComponentType,
        distance: Option<u64>,
    ) -> u64 {
        let Some(component) = self.component(ty) else {
            return 0;
        };

        match component {
            ComponentManifest::Single(_) | ComponentManifest::Files(_) => {
                component.total_output_size()
            }
            ComponentManifest::Chunked(chunked) => {
                let num_chunks = chunked.num_chunks();
                let start_chunk = self.start_chunk_for_distance(chunked, distance);

                (start_chunk..num_chunks)
                    .map(|index| chunked.chunk_output_size(index as usize))
                    .sum()
            }
        }
    }

    /// Returns the number of archives that would be downloaded for a distance ending at
    /// [`Self::block`].
    pub fn chunks_for_distance(&self, ty: SnapshotComponentType, distance: Option<u64>) -> u64 {
        match self.component(ty) {
            None => 0,
            Some(ComponentManifest::Single(_)) => 1,
            Some(ComponentManifest::Files(set)) => set.archives.len() as u64,
            Some(ComponentManifest::Chunked(chunked)) => {
                chunked.num_chunks() - self.start_chunk_for_distance(chunked, distance)
            }
        }
    }

    /// Returns the first chunk needed for a distance measured from the declared snapshot block.
    fn start_chunk_for_distance(&self, chunked: &ChunkedArchive, distance: Option<u64>) -> u64 {
        let Some(distance) = distance else { return 0 };
        // The final archive may extend past the snapshot block. Counting back from its end
        // would shift the requested history window forward and could skip a required chunk.
        let first_block = self.block.saturating_sub(distance.saturating_sub(1));
        // Legacy manifests used an inclusive block height for total_blocks. At equality,
        // conservatively retain the boundary chunk if it exists.
        if distance == 0 || first_block > chunked.total_blocks {
            return chunked.num_chunks();
        }
        first_block / chunked.blocks_per_file
    }

    /// Fails if any chunked component declares zero blocks per archive.
    pub(crate) fn validate_blocks_per_file(&self) -> Result<()> {
        for (key, component) in &self.components {
            if let ComponentManifest::Chunked(chunked) = component {
                eyre::ensure!(
                    chunked.blocks_per_file > 0,
                    "Invalid modular manifest: {key} blocks_per_file must be greater than zero"
                );
            }
        }
        Ok(())
    }
}

impl ComponentManifest {
    /// Returns the total download size for this component.
    pub fn total_size(&self) -> u64 {
        match self {
            Self::Single(s) => s.size,
            Self::Chunked(c) => c.chunk_sizes.iter().sum(),
            Self::Files(set) => set.archives.iter().map(|archive| archive.size).sum(),
        }
    }

    /// Returns the total extracted plain-output size for this component.
    pub fn total_output_size(&self) -> u64 {
        match self {
            Self::Single(single) => single.output_size(),
            Self::Chunked(chunked) => chunked.total_output_size(),
            Self::Files(set) => set.archives.iter().map(SingleArchive::output_size).sum(),
        }
    }
}

impl ChunkedArchive {
    /// Returns the number of chunks.
    pub fn num_chunks(&self) -> u64 {
        self.total_blocks.div_ceil(self.blocks_per_file)
    }

    /// Returns the number of tail chunks required to cover at least `distance` blocks from the
    /// tip.
    pub fn tail_chunks_for_distance(&self, distance: u64) -> u64 {
        let needed = distance.min(self.total_blocks);
        if needed == 0 {
            return 0;
        }

        // The first needed block determines the earliest chunk, including for a partial tail.
        let first_chunk = (self.total_blocks - needed) / self.blocks_per_file;
        self.num_chunks() - first_chunk
    }

    /// Returns `true` when `chunk_files` is empty or has exactly one path per chunk.
    pub fn chunk_files_are_consistent(&self) -> bool {
        self.chunk_files.is_empty() || self.chunk_files.len() as u64 == self.num_chunks()
    }

    /// Returns the archive location for chunk `index`: a path relative to the manifest base URL,
    /// or an absolute URL for an archive reused from a previous snapshot.
    ///
    /// Uses [`Self::chunk_files`] when it has exactly one entry per chunk; otherwise the
    /// default `{key}-{start}-{end}.tar.zst` name.
    pub fn chunk_relative_path(&self, key: &str, index: u64) -> String {
        if self.chunk_files.len() as u64 == self.num_chunks() &&
            let Some(path) = self.chunk_files.get(index as usize)
        {
            return path.clone();
        }
        let start = index * self.blocks_per_file;
        let end = (index + 1) * self.blocks_per_file - 1;
        format!("{key}-{start}-{end}.tar.zst")
    }

    /// Returns the extracted plain-output size for one chunk.
    pub fn chunk_output_size(&self, index: usize) -> u64 {
        self.chunk_decompressed_sizes.get(index).copied().unwrap_or_else(|| {
            self.chunk_output_files
                .get(index)
                .map(|files| files.iter().map(|file| file.size).sum())
                .unwrap_or(0)
        })
    }

    /// Returns the total extracted plain-output size across all chunks.
    pub fn total_output_size(&self) -> u64 {
        if !self.chunk_decompressed_sizes.is_empty() {
            self.chunk_decompressed_sizes.iter().sum()
        } else {
            self.chunk_output_files
                .iter()
                .map(|files| files.iter().map(|file| file.size).sum::<u64>())
                .sum()
        }
    }
}

/// Joins an archive path relative to `base_url`.
///
/// Ensures directory semantics for `base_url` so the last path segment is not replaced when
/// joining nested paths like `static_files/headers-0-499999.tar.zst`. Leading slashes on
/// `relative_path` are stripped so publisher paths stay relative to the base prefix. An absolute
/// URL, such as an archive reused from a previous snapshot, is returned unchanged.
fn resolve_archive_url(base_url: &str, relative_path: &str) -> String {
    if Url::parse(relative_path).is_ok() {
        return relative_path.to_string();
    }

    let relative_path = relative_path.trim_start_matches('/');

    if base_url.is_empty() {
        return relative_path.to_string();
    }

    let Ok(mut base) = Url::parse(base_url) else {
        return format!("{}/{}", base_url.trim_end_matches('/'), relative_path);
    };

    // Url::join replaces the final path segment unless the base path ends with `/`.
    let path = base.path();
    if !path.ends_with('/') {
        let mut with_slash = path.to_string();
        with_slash.push('/');
        base.set_path(&with_slash);
    }

    match base.join(relative_path) {
        Ok(joined) => joined.to_string(),
        Err(_) => format!("{}/{}", base_url.trim_end_matches('/'), relative_path),
    }
}

impl SingleArchive {
    /// Returns the total extracted plain-output size for this archive.
    pub fn output_size(&self) -> u64 {
        if self.decompressed_size != 0 {
            self.decompressed_size
        } else {
            self.output_files.iter().map(|file| file.size).sum()
        }
    }

    /// Returns the concrete download for this archive, resolved against `base_url`.
    fn snapshot_archive(&self, base_url: &str) -> SnapshotArchive {
        SnapshotArchive {
            url: resolve_archive_url(base_url, &self.file),
            file_name: self.file.clone(),
            size: self.size,
            blake3: self.blake3.clone(),
            output_files: self.output_files.clone(),
        }
    }
}

/// Fetches a snapshot manifest from a URL, rejecting zero blocks per archive.
pub async fn fetch_manifest(manifest_url: &str) -> Result<SnapshotManifest> {
    let client = Client::new();
    let manifest: SnapshotManifest =
        client.get(manifest_url).send().await?.error_for_status()?.json().await?;
    manifest.validate_blocks_per_file()?;
    Ok(manifest)
}

/// Package chunk archives from a source datadir and generate a manifest.
///
/// `block` selects chunks whose nominal ranges start at or before the snapshot height. Each
/// selected file is archived in full; component block counts come from its actual stored ranges.
///
/// With a `previous` manifest, a chunk that is not the last chunk of its component in either
/// manifest is not packaged again: its source files are hashed and must match the previous
/// manifest's `chunk_output_files`, and the new manifest points `chunk_files` at the
/// previously published archive. A mismatch is an error, since sealed history must not change.
/// Likewise, a RocksDB SST file whose path and checksum match an SST archive in the previous
/// manifest is not packaged again and points at the previously published archive. SST files
/// that are new or changed are packaged, since compaction replaces them between snapshots.
pub fn generate_manifest(
    source_datadir: &Path,
    output_dir: &Path,
    base_url: Option<&str>,
    block: u64,
    chain_id: u64,
    blocks_per_file: u64,
    previous: Option<&SnapshotManifest>,
) -> Result<SnapshotManifest> {
    eyre::ensure!(blocks_per_file > 0, "blocks_per_file must be greater than zero");
    if let Some(previous) = previous {
        eyre::ensure!(
            previous.chain_id == chain_id,
            "Previous manifest is for chain {}, not {chain_id}",
            previous.chain_id
        );
        // Reused archives are referenced by absolute URL, which needs an absolute base URL.
        eyre::ensure!(
            previous.base_url.as_deref().is_some_and(|url| Url::parse(url).is_ok()),
            "Previous manifest has no absolute base_url to locate its archives"
        );
    }

    std::fs::create_dir_all(output_dir)?;

    let mut components = BTreeMap::new();
    let mut static_files_dir = source_datadir.join("static_files");
    if !static_files_dir.exists() {
        static_files_dir = source_datadir.to_path_buf();
    }

    let static_files = iter_static_files(&static_files_dir)?;

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
        let segment = static_segment(*ty).expect("chunked components have a static file segment");
        let Some((last_range, _)) = static_files.get(segment).and_then(|ranges| {
            ranges.iter().rfind(|(_, header)| header.expected_block_range().start() <= block)
        }) else {
            continue;
        };

        // Coverage starts at block zero, and selected files are archived in full, even past
        // `block`.
        let total_blocks = last_range
            .end()
            .checked_add(1)
            .ok_or_else(|| eyre::eyre!("Block count overflows for {key}"))?;

        let num_chunks = total_blocks.div_ceil(blocks_per_file);
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
            // Chunk indices only match the previous manifest's positions without leading gaps.
            let previous_chunks = previous
                .filter(|_| planned_chunks.len() as u64 == num_chunks)
                .and_then(|manifest| match manifest.component(*ty)? {
                    ComponentManifest::Chunked(chunked) => Some((manifest, chunked)),
                    ComponentManifest::Single(_) | ComponentManifest::Files(_) => None,
                })
                // The width check comes first: the other checks divide by the previous width.
                .filter(|(_, chunked)| {
                    chunked.blocks_per_file == blocks_per_file &&
                        chunked.chunk_files_are_consistent() &&
                        chunked.chunk_sizes.len() as u64 == chunked.num_chunks() &&
                        chunked.chunk_output_files.len() as u64 == chunked.num_chunks()
                });

            // The last chunk of either manifest may be partial, so only earlier chunks are reused.
            let reusable_chunks = previous_chunks
                .map_or(0, |(_, chunked)| num_chunks.min(chunked.num_chunks()).saturating_sub(1));

            let mut packaged_chunks = planned_chunks
                .into_par_iter()
                .map(|planned| -> Result<PackagedChunk> {
                    let start = planned.chunk_idx * blocks_per_file;
                    let file_name = chunk_filename(key, start, start + blocks_per_file - 1);
                    if planned.chunk_idx < reusable_chunks &&
                        let Some((manifest, chunked)) = previous_chunks
                    {
                        let idx = planned.chunk_idx as usize;
                        let output_files = hash_chunk_files(&planned.source_files)?;
                        eyre::ensure!(
                            output_files == chunked.chunk_output_files[idx],
                            "{file_name} differs from the previous manifest"
                        );

                        return Ok(PackagedChunk {
                            chunk_idx: planned.chunk_idx,
                            size: chunked.chunk_sizes[idx],
                            output_files,
                            file: resolve_archive_url(
                                manifest.base_url_or_empty(),
                                &chunked.chunk_relative_path(key, planned.chunk_idx),
                            ),
                        });
                    }

                    let output_files =
                        write_chunk_archive(&planned.archive_path, &planned.source_files)?;
                    let size = std::fs::metadata(&planned.archive_path)?.len();
                    Ok(PackagedChunk {
                        chunk_idx: planned.chunk_idx,
                        size,
                        output_files,
                        file: file_name,
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .collect::<Result<Vec<_>>>()?;

            packaged_chunks.sort_unstable_by_key(|chunk| chunk.chunk_idx);
            let chunk_files = packaged_chunks.iter().map(|chunk| chunk.file.clone()).collect();
            let chunk_sizes = packaged_chunks.iter().map(|chunk| chunk.size).collect::<Vec<_>>();
            let chunk_output_files =
                packaged_chunks.into_iter().map(|chunk| chunk.output_files).collect::<Vec<_>>();
            let total_size: u64 = chunk_sizes.iter().sum();
            info!(target: "reth::cli",
                component = ty.display_name(),
                chunks = chunk_sizes.len(),
                total_blocks,
                size = %super::DownloadProgress::format_size(total_size),
                "Found chunked component"
            );
            components.insert(
                key.to_string(),
                ComponentManifest::Chunked(ChunkedArchive {
                    blocks_per_file,
                    total_blocks,
                    chunk_sizes,
                    chunk_decompressed_sizes: chunk_output_files
                        .iter()
                        .map(|files| files.iter().map(|file| file.size).sum())
                        .collect(),
                    chunk_files,
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
            decompressed_size: state_output_files.iter().map(|file| file.size).sum(),
            blake3: None,
            output_files: state_output_files,
        }),
    );

    let rocksdb_files = rocksdb_source_files(source_datadir)?;
    if !rocksdb_files.is_empty() {
        let archives = package_rocksdb_archives(output_dir, rocksdb_files, previous)?;
        info!(target: "reth::cli",
            archives = archives.len(),
            size = %super::DownloadProgress::format_size(
                archives.iter().map(|archive| archive.size).sum()
            ),
            "Found RocksDB indices"
        );
        components.insert(
            SnapshotComponentType::RocksdbIndices.key().to_string(),
            ComponentManifest::Files(ArchiveSet { archives }),
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
        extensions: Default::default(),
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
    /// Entry for `chunk_files`: the published URL when reused, otherwise the local file name.
    file: String,
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
    let Some(segment) = static_segment(component) else {
        return Ok(Vec::new());
    };

    let static_files_dir = source_datadir.join("static_files");
    let static_files_dir =
        if static_files_dir.exists() { static_files_dir } else { source_datadir.to_path_buf() };
    let prefix = format!("static_file_{}_{start}_{end}", segment.as_str());

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

fn static_segment(component: SnapshotComponentType) -> Option<StaticFileSegment> {
    match component {
        SnapshotComponentType::Headers => Some(StaticFileSegment::Headers),
        SnapshotComponentType::Transactions => Some(StaticFileSegment::Transactions),
        SnapshotComponentType::TransactionSenders => Some(StaticFileSegment::TransactionSenders),
        SnapshotComponentType::Receipts => Some(StaticFileSegment::Receipts),
        SnapshotComponentType::AccountChangesets => Some(StaticFileSegment::AccountChangeSets),
        SnapshotComponentType::StorageChangesets => Some(StaticFileSegment::StorageChangeSets),
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

    eyre::bail!("Could not find source state DB directory under {}", source_datadir.display());
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

/// Packages each RocksDB SST file into its own archive and the remaining files into one.
///
/// SST archives are written to `rocksdb/{file_stem}-{blake3}.tar.zst`, so an unchanged SST gets
/// the same archive name in every snapshot and publishers can upload it once. The file stem
/// keeps the final path segment unique, since downloads are cached by that segment.
///
/// An SST file that matches a single-file archive in the `previous` manifest's RocksDB indices is
/// not packaged; its archive entry points at the previously published archive instead.
fn package_rocksdb_archives(
    output_dir: &Path,
    files: Vec<PlannedFile>,
    previous: Option<&SnapshotManifest>,
) -> Result<Vec<SingleArchive>> {
    // Only SST files directly under the RocksDB directory get their own archive: their file
    // names are unique there, while nested directories may repeat them.
    let (sst_files, other_files): (Vec<_>, Vec<_>) = files.into_iter().partition(|file| {
        file.relative_path.parent() == Some(Path::new("rocksdb")) &&
            file.relative_path.extension().is_some_and(|ext| ext == "sst")
    });

    // Published SST archives by the file they extract, with absolute URLs.
    let mut published = HashMap::new();
    if let Some(previous) = previous &&
        let Some(ComponentManifest::Files(set)) =
            previous.component(SnapshotComponentType::RocksdbIndices)
    {
        for archive in &set.archives {
            if let [output] = archive.output_files.as_slice() {
                published.insert(
                    output.clone(),
                    SingleArchive {
                        file: resolve_archive_url(previous.base_url_or_empty(), &archive.file),
                        ..archive.clone()
                    },
                );
            }
        }
    }

    let sst_dir = output_dir.join("rocksdb");
    std::fs::create_dir_all(&sst_dir)?;
    let mut archives = sst_files
        .into_par_iter()
        .map(|planned| -> Result<SingleArchive> {
            if !published.is_empty() &&
                let Some(archive) = published.get(&hash_planned_file(&planned)?)
            {
                return Ok(archive.clone());
            }

            let stem = planned
                .relative_path
                .file_stem()
                .ok_or_else(|| {
                    eyre::eyre!("Invalid RocksDB file path: {}", planned.relative_path.display())
                })?
                .to_string_lossy()
                .to_string();
            // The archive name depends on the checksum, which is known once the file is read.
            let temp_path = sst_dir.join(format!("{stem}.tar.zst.tmp"));
            let output_files =
                write_archive_from_planned_files(&temp_path, std::slice::from_ref(&planned))?;
            let output = &output_files[0];
            let file = format!("rocksdb/{stem}-{}.tar.zst", output.blake3);
            std::fs::rename(&temp_path, output_dir.join(&file))?;
            Ok(SingleArchive {
                size: std::fs::metadata(output_dir.join(&file))?.len(),
                decompressed_size: output.size,
                file,
                blake3: None,
                output_files,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // Reused archives are absolute URLs, so order by the SST each archive extracts.
    archives.sort_unstable_by(|a, b| a.output_files[0].path.cmp(&b.output_files[0].path));

    if !other_files.is_empty() {
        let file = "rocksdb_indices.tar.zst";
        let (size, output_files) = package_single_component(output_dir, file, &other_files)?;
        archives.push(SingleArchive {
            file: file.to_string(),
            size,
            decompressed_size: output_files.iter().map(|file| file.size).sum(),
            blake3: None,
            output_files,
        });
    }

    Ok(archives)
}

fn write_chunk_archive(path: &Path, source_files: &[PathBuf]) -> Result<Vec<OutputFileChecksum>> {
    write_archive_from_planned_files(path, &chunk_planned_files(source_files)?)
}

/// Computes the output checksums a chunk archive of these files would record, without packaging.
fn hash_chunk_files(source_files: &[PathBuf]) -> Result<Vec<OutputFileChecksum>> {
    chunk_planned_files(source_files)?.iter().map(hash_planned_file).collect()
}

/// Computes the output checksum an archive would record for this file, without packaging.
fn hash_planned_file(planned: &PlannedFile) -> Result<OutputFileChecksum> {
    let mut reader = HashingReader::new(reth_fs_util::open(&planned.source_path)?);
    std::io::copy(&mut reader, &mut std::io::sink())?;
    Ok(OutputFileChecksum {
        path: planned.relative_path.to_string_lossy().to_string(),
        size: reader.bytes_read,
        blake3: reader.finalize(),
    })
}

fn chunk_planned_files(source_files: &[PathBuf]) -> Result<Vec<PlannedFile>> {
    source_files
        .iter()
        .map(|source_path| {
            let file_name = source_path.file_name().ok_or_else(|| {
                eyre::eyre!("Invalid source file path: {}", source_path.display())
            })?;
            Ok(PlannedFile {
                source_path: source_path.clone(),
                relative_path: PathBuf::from("static_files").join(file_name),
            })
        })
        .collect()
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
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_node_api::NodeTypes;
    use reth_provider::{test_utils::MockNodeTypes, StaticFileProviderBuilder, StaticFileWriter};
    use tempfile::tempdir;

    fn test_manifest() -> SnapshotManifest {
        let mut components = BTreeMap::new();
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size: 100,
                decompressed_size: 0,
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
                chunk_decompressed_sizes: vec![],
                chunk_files: vec![],
                chunk_output_files: vec![vec![], vec![], vec![]],
            }),
        );
        components.insert(
            "headers".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_500_000,
                chunk_sizes: vec![40_000, 50_000, 60_000],
                chunk_decompressed_sizes: vec![],
                chunk_files: vec![],
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
            extensions: Default::default(),
        }
    }

    fn snapshot_source() -> tempfile::TempDir {
        let source = tempdir().unwrap();
        reth_fs_util::create_dir_all(source.path().join("db")).unwrap();
        reth_fs_util::write(source.path().join("db/mdbx.dat"), b"state-data").unwrap();
        source
    }

    fn write_test_static_files(source: &Path, segment: StaticFileSegment, tip: u64) {
        let static_files = source.join("static_files");
        reth_fs_util::create_dir_all(&static_files).unwrap();

        let provider = StaticFileProviderBuilder::read_write(&static_files)
            .with_blocks_per_file(10)
            .build::<<MockNodeTypes as NodeTypes>::Primitives>()
            .unwrap();

        let mut writer = provider.get_writer(0, segment).unwrap();
        for number in 0..=tip {
            if segment == StaticFileSegment::Headers {
                writer
                    .append_header(&Header { number, ..Default::default() }, &B256::ZERO)
                    .unwrap();
            } else {
                writer.increment_block(number).unwrap();
            }
        }

        writer.commit().unwrap();
    }

    #[test]
    fn manifest_preserves_extensions() {
        let manifest: SnapshotManifest = serde_json::from_str(
            r#"{
                "block": 1,
                "chain_id": 1,
                "storage_version": 2,
                "timestamp": 0,
                "components": {},
                "consensus": { "archive": "consensus.tar.zst" }
            }"#,
        )
        .unwrap();

        assert_eq!(
            manifest.extensions.get("consensus"),
            Some(&serde_json::json!({ "archive": "consensus.tar.zst" }))
        );
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
                decompressed_size: 0,
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
            extensions: Default::default(),
        };

        let urls = m.archive_urls_for_distance(SnapshotComponentType::RocksdbIndices, Some(10));
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com/rocksdb_indices.tar.zst");
        assert_eq!(m.size_for_distance(SnapshotComponentType::RocksdbIndices, Some(10)), 777);
    }

    #[test]
    fn generate_manifest_reuses_unchanged_rocksdb_ssts_from_previous_manifest() {
        let source = tempdir().unwrap();
        let db_dir = source.path().join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::write(db_dir.join("mdbx.dat"), b"state-data").unwrap();
        let rocksdb_dir = source.path().join("rocksdb");
        std::fs::create_dir_all(&rocksdb_dir).unwrap();
        std::fs::write(rocksdb_dir.join("CURRENT"), b"MANIFEST-000001").unwrap();
        std::fs::write(rocksdb_dir.join("000007.sst"), b"sst-7").unwrap();
        std::fs::write(rocksdb_dir.join("000009.sst"), b"sst-9").unwrap();
        let hash = |contents: &[u8]| blake3::hash(contents).to_hex().to_string();
        let rocksdb_archives = |manifest: &SnapshotManifest| {
            let Some(ComponentManifest::Files(set)) =
                manifest.component(SnapshotComponentType::RocksdbIndices)
            else {
                panic!("rocksdb indices should be an archive set")
            };
            set.archives.clone()
        };

        let previous = generate_manifest(
            source.path(),
            tempdir().unwrap().path(),
            Some("https://example.com/previous"),
            0,
            1,
            500_000,
            None,
        )
        .unwrap();
        let previous_archives = rocksdb_archives(&previous);

        // 000007 is unchanged, 000009 is rewritten in place, and 000011 is new.
        std::fs::write(rocksdb_dir.join("000009.sst"), b"sst-9-compacted").unwrap();
        std::fs::write(rocksdb_dir.join("000011.sst"), b"sst-11").unwrap();
        let output = tempdir().unwrap();
        let mut next =
            generate_manifest(source.path(), output.path(), None, 0, 1, 500_000, Some(&previous))
                .unwrap();
        let archives = rocksdb_archives(&next);

        let reused_sst = format!("rocksdb/000007-{}.tar.zst", hash(b"sst-7"));
        let files = archives.iter().map(|archive| archive.file.as_str()).collect::<Vec<_>>();
        assert_eq!(
            files,
            [
                format!("https://example.com/previous/{reused_sst}"),
                format!("rocksdb/000009-{}.tar.zst", hash(b"sst-9-compacted")),
                format!("rocksdb/000011-{}.tar.zst", hash(b"sst-11")),
                "rocksdb_indices.tar.zst".to_string(),
            ]
        );
        assert_eq!(archives[0].size, previous_archives[0].size);
        assert_eq!(archives[0].output_files, previous_archives[0].output_files);
        assert!(!output.path().join(&reused_sst).exists());
        for archive in &archives[1..] {
            assert!(output.path().join(&archive.file).exists(), "{}", archive.file);
        }

        // An archive reused from an older manifest keeps its absolute URL in later manifests.
        next.base_url = Some("https://example.com/next".to_string());
        let later = generate_manifest(
            source.path(),
            tempdir().unwrap().path(),
            None,
            0,
            1,
            500_000,
            Some(&next),
        )
        .unwrap();
        let later_archives = rocksdb_archives(&later);
        assert_eq!(later_archives[0].file, format!("https://example.com/previous/{reused_sst}"));
        assert_eq!(
            later_archives[2].file,
            format!("https://example.com/next/rocksdb/000011-{}.tar.zst", hash(b"sst-11"))
        );
    }

    #[test]
    fn rocksdb_archive_set_resolves_every_archive() {
        let json = r#"{
            "block": 1,
            "chain_id": 1,
            "storage_version": 2,
            "timestamp": 0,
            "base_url": "https://example.com/1700000",
            "components": {
                "rocksdb_indices": {
                    "archives": [
                        {
                            "file": "../rocksdb/000007-aa.tar.zst",
                            "size": 10,
                            "decompressed_size": 100,
                            "output_files": [
                                {"path": "rocksdb/000007.sst", "size": 100, "blake3": "aa"}
                            ]
                        },
                        {
                            "file": "rocksdb_indices.tar.zst",
                            "size": 2,
                            "output_files": [
                                {"path": "rocksdb/CURRENT", "size": 15, "blake3": "bb"}
                            ]
                        }
                    ]
                }
            }
        }"#;
        let m: SnapshotManifest = serde_json::from_str(json).unwrap();
        let ty = SnapshotComponentType::RocksdbIndices;
        assert!(matches!(m.component(ty), Some(ComponentManifest::Files(_))));
        m.validate_output_paths().unwrap();

        let urls = [
            "https://example.com/rocksdb/000007-aa.tar.zst",
            "https://example.com/1700000/rocksdb_indices.tar.zst",
        ];
        assert_eq!(m.archive_urls(ty), urls);
        assert_eq!(m.archive_urls_for_distance(ty, Some(10)), urls);
        let archives = m.snapshot_archives_for_distance(ty, Some(10));
        assert_eq!(archives.iter().map(|archive| archive.url.as_str()).collect::<Vec<_>>(), urls);
        assert_eq!(archives[0].output_files[0].path, "rocksdb/000007.sst");
        assert_eq!(m.chunks_for_distance(ty, Some(10)), 2);
        assert_eq!(m.size_for_distance(ty, Some(10)), 12);
        assert_eq!(m.total_size(&[ty]), 12);
        assert_eq!(m.output_size_for_distance(ty, Some(10)), 115);
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

    /// A snapshot whose final chunk is partial: `total_blocks` is not a multiple of
    /// `blocks_per_file`, so the last chunk holds fewer than `blocks_per_file` blocks.
    /// Three chunks: `[0, 500k)`, `[500k, 1M)`, and `[1M, 1.005M)` — the last holds 5_000 blocks.
    fn partial_tail_manifest() -> SnapshotManifest {
        let mut components = BTreeMap::new();
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_005_000,
                chunk_sizes: vec![10, 20, 30],
                chunk_decompressed_sizes: vec![100, 200, 300],
                chunk_files: vec![],
                chunk_output_files: vec![vec![], vec![], vec![]],
            }),
        );
        SnapshotManifest {
            block: 1_005_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
            components,
            extensions: Default::default(),
        }
    }

    #[test]
    fn tail_chunks_for_distance_accounts_for_partial_final_chunk() {
        let chunked = ChunkedArchive {
            blocks_per_file: 500_000,
            total_blocks: 1_005_000,
            chunk_sizes: vec![10, 20, 30],
            chunk_decompressed_sizes: vec![],
            chunk_files: vec![],
            chunk_output_files: vec![],
        };
        // The final chunk holds only 5_000 blocks, so covering 10_064 blocks needs the last two
        // chunks. The naive `distance.div_ceil(blocks_per_file)` returns 1 here and falls short.
        assert_eq!(chunked.tail_chunks_for_distance(10_064), 2);
        // A distance that fits inside the final chunk still needs just one.
        assert_eq!(chunked.tail_chunks_for_distance(5_000), 1);
        // One block more than the final chunk holds must pull in the previous chunk.
        assert_eq!(chunked.tail_chunks_for_distance(5_001), 2);
        assert_eq!(chunked.tail_chunks_for_distance(1), 1);
        // Covering everything selects all chunks and never exceeds the chunk count.
        assert_eq!(chunked.tail_chunks_for_distance(1_005_000), 3);
        assert_eq!(chunked.tail_chunks_for_distance(u64::MAX), 3);
    }

    #[test]
    fn partial_final_chunk_downloads_enough_history() {
        let m = partial_tail_manifest();
        // The published snapshot's final chunk is partial (5_000 blocks). A --full download
        // asking for 10_064 blocks must pull the last two chunks, then prune the surplus,
        // rather than the single short tail chunk the old distance math selected.
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::Transactions, Some(10_064)), 2);

        let urls = m.archive_urls_for_distance(SnapshotComponentType::Transactions, Some(10_064));
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com/transactions-500000-999999.tar.zst");
        assert_eq!(urls[1], "https://example.com/transactions-1000000-1499999.tar.zst");

        let archives =
            m.snapshot_archives_for_distance(SnapshotComponentType::Transactions, Some(10_064));
        assert_eq!(archives.len(), 2);

        // Reported sizes cover the selected tail chunks (indices 1 and 2).
        assert_eq!(m.size_for_distance(SnapshotComponentType::Transactions, Some(10_064)), 20 + 30);
        assert_eq!(
            m.output_size_for_distance(SnapshotComponentType::Transactions, Some(10_064)),
            200 + 300
        );

        // A distance that fits inside the final chunk still selects a single chunk.
        assert_eq!(m.chunks_for_distance(SnapshotComponentType::Transactions, Some(5_000)), 1);
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
        assert_eq!(ComponentSelection::Since(15_537_394).to_string(), "Since block 15537394");
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
                chunk_decompressed_sizes: vec![],
                chunk_files: vec![],
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
            extensions: Default::default(),
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
    fn manifest_output_paths_must_stay_relative() {
        fn output(path: &str) -> OutputFileChecksum {
            OutputFileChecksum { path: path.into(), size: 0, blake3: String::new() }
        }

        let mut manifest = test_manifest();
        let mut with_outputs = |single: &[&str], chunks: &[&str]| {
            manifest.components.insert(
                "state".into(),
                ComponentManifest::Single(SingleArchive {
                    file: "state.tar.zst".into(),
                    size: 1,
                    decompressed_size: 0,
                    blake3: None,
                    output_files: single.iter().map(|path| output(path)).collect(),
                }),
            );
            manifest.components.insert(
                "transactions".into(),
                ComponentManifest::Chunked(ChunkedArchive {
                    blocks_per_file: 500_000,
                    total_blocks: 500_000,
                    chunk_sizes: vec![1],
                    chunk_decompressed_sizes: vec![],
                    chunk_files: vec![],
                    chunk_output_files: vec![chunks.iter().map(|path| output(path)).collect()],
                }),
            );
            manifest.validate_output_paths()
        };

        with_outputs(&["db/mdbx.dat", "./static_files/headers"], &["static_files/a/b"]).unwrap();
        for bad in ["/etc/passwd", "../outside", "db/../../outside", "db/..", ".", "", "./"] {
            let err = with_outputs(&[bad], &[]).unwrap_err();
            assert!(format!("{err:#}").contains("Invalid manifest output file path"), "{bad}");
            assert!(with_outputs(&[], &[bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn output_size_for_distance_uses_manifest_or_output_files() {
        let m = test_manifest();
        assert_eq!(m.output_size_for_distance(SnapshotComponentType::Transactions, None), 0);

        let mut components = BTreeMap::new();
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size: 100,
                decompressed_size: 1_000,
                blake3: None,
                output_files: vec![OutputFileChecksum {
                    path: "db/mdbx.dat".to_string(),
                    size: 1_000,
                    blake3: "h0".to_string(),
                }],
            }),
        );
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_000_000,
                chunk_sizes: vec![80_000, 120_000],
                chunk_decompressed_sizes: vec![111, 222],
                chunk_files: vec![],
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
        let manifest = SnapshotManifest {
            block: 1_000_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
            components,
            extensions: Default::default(),
        };

        assert_eq!(manifest.output_size_for_distance(SnapshotComponentType::State, None), 1_000);
        assert_eq!(
            manifest.output_size_for_distance(SnapshotComponentType::Transactions, None),
            333
        );
        assert_eq!(
            manifest.output_size_for_distance(SnapshotComponentType::Transactions, Some(500_000)),
            222
        );
    }

    #[test]
    fn archive_descriptors_include_checksum_metadata() {
        let mut components = BTreeMap::new();
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size: 100,
                decompressed_size: 1_000,
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
                chunk_decompressed_sizes: vec![111, 222],
                chunk_files: vec![],
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
            extensions: Default::default(),
        };

        let state = m.snapshot_archives_for_distance(SnapshotComponentType::State, None);
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].file_name, "state.tar.zst");
        assert_eq!(state[0].blake3.as_deref(), Some("abc123"));
        assert_eq!(state[0].output_files.len(), 1);

        let tx = m.snapshot_archives_for_distance(SnapshotComponentType::Transactions, None);
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
            generate_manifest(source.path(), output.path(), None, 0, 1, 500_000, None).unwrap();

        let state = manifest.component(SnapshotComponentType::State).unwrap();
        let ComponentManifest::Single(state) = state else {
            panic!("state should be a single archive")
        };
        assert_eq!(state.file, "state.tar.zst");
        assert!(state.decompressed_size > 0);
        assert!(!state.output_files.is_empty());
        assert_eq!(state.output_files[0].path, "db/mdbx.dat");
        assert!(output.path().join("state.tar.zst").exists());
    }

    #[test]
    fn generate_manifest_packages_each_rocksdb_sst_separately() {
        let source = tempdir().unwrap();
        let db_dir = source.path().join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::write(db_dir.join("mdbx.dat"), b"state-data").unwrap();
        let rocksdb_dir = source.path().join("rocksdb");
        std::fs::create_dir_all(&rocksdb_dir).unwrap();
        std::fs::write(rocksdb_dir.join("CURRENT"), b"MANIFEST-000001").unwrap();
        std::fs::write(rocksdb_dir.join("MANIFEST-000001"), b"manifest").unwrap();
        std::fs::write(rocksdb_dir.join("000007.sst"), b"sst-7").unwrap();
        std::fs::write(rocksdb_dir.join("000009.sst"), b"sst-9").unwrap();
        // A nested SST with a top-level file name stays in the shared archive.
        std::fs::create_dir_all(rocksdb_dir.join("checkpoint")).unwrap();
        std::fs::write(rocksdb_dir.join("checkpoint/000007.sst"), b"sst-7").unwrap();
        let sst_7_hash = blake3::hash(b"sst-7").to_hex().to_string();
        let sst_9_hash = blake3::hash(b"sst-9").to_hex().to_string();

        let output = tempdir().unwrap();
        let manifest =
            generate_manifest(source.path(), output.path(), None, 0, 1, 500_000, None).unwrap();

        let rocksdb = manifest.component(SnapshotComponentType::RocksdbIndices).unwrap();
        let ComponentManifest::Files(set) = rocksdb else {
            panic!("rocksdb indices should be an archive set")
        };
        let files = set.archives.iter().map(|archive| archive.file.clone()).collect::<Vec<_>>();
        assert_eq!(
            files,
            [
                format!("rocksdb/000007-{sst_7_hash}.tar.zst"),
                format!("rocksdb/000009-{sst_9_hash}.tar.zst"),
                "rocksdb_indices.tar.zst".to_string(),
            ]
        );
        for archive in &set.archives {
            assert!(output.path().join(&archive.file).exists(), "{}", archive.file);
            assert_eq!(
                archive.size,
                std::fs::metadata(output.path().join(&archive.file)).unwrap().len()
            );
        }
        assert_eq!(
            set.archives[0].output_files,
            [OutputFileChecksum {
                path: "rocksdb/000007.sst".to_string(),
                size: 5,
                blake3: sst_7_hash
            }]
        );
        assert_eq!(set.archives[0].decompressed_size, 5);
        assert_eq!(
            set.archives[2].output_files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            ["rocksdb/CURRENT", "rocksdb/MANIFEST-000001", "rocksdb/checkpoint/000007.sst"]
        );
        assert!(!output.path().join("rocksdb/000007.tar.zst.tmp").exists());
        assert_eq!(manifest.chunks_for_distance(SnapshotComponentType::RocksdbIndices, None), 3);

        // An unchanged SST keeps its archive name in a later snapshot, so publishers can reuse it.
        std::fs::remove_file(rocksdb_dir.join("000009.sst")).unwrap();
        std::fs::write(rocksdb_dir.join("000011.sst"), b"sst-11").unwrap();
        let later_output = tempdir().unwrap();
        let later =
            generate_manifest(source.path(), later_output.path(), None, 0, 1, 500_000, None)
                .unwrap();
        let Some(ComponentManifest::Files(later_set)) =
            later.component(SnapshotComponentType::RocksdbIndices)
        else {
            panic!("rocksdb indices should be an archive set")
        };
        assert_eq!(later_set.archives[0].file, set.archives[0].file);
        assert_eq!(
            later_set.archives[1].file,
            format!("rocksdb/000011-{}.tar.zst", blake3::hash(b"sst-11").to_hex())
        );
    }

    #[test]
    fn resolve_archive_url_joins_nested_paths_under_base() {
        assert_eq!(
            resolve_archive_url(
                "https://example.com/mainnet",
                "static_files/headers-0-499999.tar.zst"
            ),
            "https://example.com/mainnet/static_files/headers-0-499999.tar.zst"
        );
        assert_eq!(
            resolve_archive_url("https://example.com/mainnet/", "1700000/state.tar.zst"),
            "https://example.com/mainnet/1700000/state.tar.zst"
        );
        assert_eq!(
            resolve_archive_url("https://example.com/mainnet", "headers-0-499999.tar.zst"),
            "https://example.com/mainnet/headers-0-499999.tar.zst"
        );
    }

    #[test]
    fn chunk_files_resolve_relative_to_root_base_url() {
        let mut components = BTreeMap::new();
        components.insert(
            "headers".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_000_000,
                chunk_sizes: vec![40_000, 50_000],
                chunk_decompressed_sizes: vec![],
                chunk_files: vec![
                    "static_files/headers-0-499999.tar.zst".to_string(),
                    "1700000/headers-500000-999999.tar.zst".to_string(),
                ],
                chunk_output_files: vec![vec![], vec![]],
            }),
        );
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "1700000/state.tar.zst".to_string(),
                size: 100,
                decompressed_size: 0,
                blake3: None,
                output_files: vec![],
            }),
        );

        let m = SnapshotManifest {
            block: 1_000_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 1_700_000,
            base_url: Some("https://example.com/mainnet".to_string()),
            reth_version: None,
            components,
            extensions: Default::default(),
        };

        let urls = m.archive_urls(SnapshotComponentType::Headers);
        assert_eq!(urls.len(), 2, "exactly 2 header chunk URLs");
        assert_eq!(
            urls[0], "https://example.com/mainnet/static_files/headers-0-499999.tar.zst",
            "finalized chunk stays under static_files/"
        );
        assert_eq!(
            urls[1], "https://example.com/mainnet/1700000/headers-500000-999999.tar.zst",
            "tip chunk resolves under the run timestamp directory"
        );

        let state = m.snapshot_archives_for_distance(SnapshotComponentType::State, None);
        assert_eq!(state.len(), 1, "exactly one state archive");
        assert_eq!(
            state[0].url, "https://example.com/mainnet/1700000/state.tar.zst",
            "single archive file path joins under root base_url without ../"
        );
        assert_eq!(state[0].file_name, "1700000/state.tar.zst");

        let headers =
            m.snapshot_archives_for_distance(SnapshotComponentType::Headers, Some(500_000));
        assert_eq!(headers.len(), 1, "distance selection returns only the tip chunk");
        assert_eq!(
            headers[0].file_name, "1700000/headers-500000-999999.tar.zst",
            "file_name keeps the relative path from chunk_files"
        );
        assert_eq!(
            headers[0].url,
            "https://example.com/mainnet/1700000/headers-500000-999999.tar.zst"
        );
    }

    #[test]
    fn chunk_files_absent_keeps_default_chunk_names() {
        let m = test_manifest();
        let archives = m.snapshot_archives_for_distance(SnapshotComponentType::Transactions, None);
        assert_eq!(archives.len(), 3);
        assert_eq!(archives[0].file_name, "transactions-0-499999.tar.zst");
        assert_eq!(archives[0].url, "https://example.com/transactions-0-499999.tar.zst");
    }

    #[test]
    fn resolve_archive_url_normalizes_parent_dirs_and_leading_slashes() {
        assert_eq!(
            resolve_archive_url("https://example.com/static_files", "../1700000/state.tar.zst"),
            "https://example.com/1700000/state.tar.zst",
            "../ under a static_files base resolves to the run directory"
        );
        assert_eq!(
            resolve_archive_url(
                "https://example.com/mainnet",
                "/static_files/headers-0-499999.tar.zst"
            ),
            "https://example.com/mainnet/static_files/headers-0-499999.tar.zst",
            "leading slash is stripped so the base path prefix is preserved"
        );
    }

    #[test]
    fn resolve_archive_url_keeps_absolute_urls() {
        let url = "https://example.com/previous/headers-0-9.tar.zst";
        for base_url in ["", "https://example.com/mainnet", "not-a-url"] {
            assert_eq!(resolve_archive_url(base_url, url), url, "base_url {base_url:?}");
        }
    }

    #[test]
    fn mismatched_chunk_files_length_falls_back_to_default_names() {
        let mut components = BTreeMap::new();
        components.insert(
            "headers".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_000_000,
                chunk_sizes: vec![40_000, 50_000],
                chunk_decompressed_sizes: vec![],
                // Only one entry for two chunks — ignored in favor of default names.
                chunk_files: vec!["static_files/headers-0-499999.tar.zst".to_string()],
                chunk_output_files: vec![vec![], vec![]],
            }),
        );
        let m = SnapshotManifest {
            block: 1_000_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com/mainnet".to_string()),
            reth_version: None,
            components,
            extensions: Default::default(),
        };

        let ComponentManifest::Chunked(chunked) =
            m.component(SnapshotComponentType::Headers).unwrap()
        else {
            panic!("headers should be chunked");
        };
        assert!(!chunked.chunk_files_are_consistent());

        let urls = m.archive_urls(SnapshotComponentType::Headers);
        assert_eq!(urls.len(), 2);
        assert_eq!(
            urls[0], "https://example.com/mainnet/headers-0-499999.tar.zst",
            "mismatched chunk_files must not partially apply"
        );
        assert_eq!(urls[1], "https://example.com/mainnet/headers-500000-999999.tar.zst");
    }

    #[test]
    fn generate_manifest_uses_actual_component_block_ranges() {
        let source = snapshot_source();
        let output = tempdir().unwrap();
        write_test_static_files(source.path(), StaticFileSegment::Headers, 24);
        write_test_static_files(source.path(), StaticFileSegment::Transactions, 21);

        // The declared height is beyond both components' actual tips, within the same chunk.
        let manifest =
            generate_manifest(source.path(), output.path(), None, 29, 1, 10, None).unwrap();
        assert_eq!(manifest.block, 29);
        for (component, expected_blocks) in
            [(SnapshotComponentType::Headers, 25), (SnapshotComponentType::Transactions, 22)]
        {
            let ComponentManifest::Chunked(chunked) = manifest.component(component).unwrap() else {
                panic!("component should be chunked");
            };

            assert_eq!(chunked.total_blocks, expected_blocks);
            assert_eq!(chunked.num_chunks(), 3);
            assert_eq!(chunked.chunk_sizes.len(), 3);
            assert_eq!(chunked.chunk_output_files.len(), 3);
            assert_eq!(chunked.tail_chunks_for_distance(6), 2);
            for (index, size) in chunked.chunk_sizes.iter().enumerate() {
                let path =
                    output.path().join(chunked.chunk_relative_path(component.key(), index as u64));
                assert_eq!(reth_fs_util::metadata(path).unwrap().len(), *size);
            }
        }
    }

    #[test]
    fn generate_manifest_includes_genesis_and_chunk_boundary_blocks() {
        for tip in [0, 9, 10, 19, 20] {
            let source = snapshot_source();
            let output = tempdir().unwrap();
            write_test_static_files(source.path(), StaticFileSegment::Headers, tip);

            let manifest =
                generate_manifest(source.path(), output.path(), None, tip, 1, 10, None).unwrap();
            let ComponentManifest::Chunked(chunked) =
                manifest.component(SnapshotComponentType::Headers).unwrap()
            else {
                panic!("headers should be chunked");
            };

            assert_eq!(chunked.total_blocks, tip + 1);
            assert_eq!(chunked.num_chunks(), tip / 10 + 1);
            assert_eq!(chunked.chunk_sizes.len() as u64, chunked.num_chunks());
        }
    }

    #[test]
    fn generate_manifest_counts_whole_selected_chunks() {
        let source = snapshot_source();
        write_test_static_files(source.path(), StaticFileSegment::Headers, 24);

        // Selecting a chunk copies its whole file, including blocks beyond the declared height.
        for (block, expected_blocks) in [(9, 10), (10, 20), (20, 25), (39, 25)] {
            let output = tempdir().unwrap();
            let manifest =
                generate_manifest(source.path(), output.path(), None, block, 1, 10, None).unwrap();

            let ComponentManifest::Chunked(chunked) =
                manifest.component(SnapshotComponentType::Headers).unwrap()
            else {
                panic!("headers should be chunked");
            };

            assert_eq!(chunked.total_blocks, expected_blocks);
            assert_eq!(chunked.chunk_sizes.len() as u64, chunked.num_chunks());
        }
    }

    #[test]
    fn generate_manifest_reuses_sealed_chunks_from_previous_manifest() {
        let previous_source = snapshot_source();
        let previous_output = tempdir().unwrap();
        write_test_static_files(previous_source.path(), StaticFileSegment::Headers, 24);
        let previous = generate_manifest(
            previous_source.path(),
            previous_output.path(),
            Some("https://example.com/previous"),
            24,
            1,
            10,
            None,
        )
        .unwrap();

        let source = snapshot_source();
        let output = tempdir().unwrap();
        write_test_static_files(source.path(), StaticFileSegment::Headers, 34);
        let manifest =
            generate_manifest(source.path(), output.path(), None, 34, 1, 10, Some(&previous))
                .unwrap();

        let ComponentManifest::Chunked(previous_headers) =
            previous.component(SnapshotComponentType::Headers).unwrap()
        else {
            panic!("headers should be chunked");
        };

        let ComponentManifest::Chunked(headers) =
            manifest.component(SnapshotComponentType::Headers).unwrap()
        else {
            panic!("headers should be chunked");
        };

        // Chunks 0 and 1 were sealed in both snapshots; chunk 2 was the previous tip.
        assert_eq!(
            manifest.archive_urls(SnapshotComponentType::Headers),
            vec![
                "https://example.com/previous/headers-0-9.tar.zst",
                "https://example.com/previous/headers-10-19.tar.zst",
                "headers-20-29.tar.zst",
                "headers-30-39.tar.zst",
            ]
        );
        assert_eq!(headers.chunk_sizes[..2], previous_headers.chunk_sizes[..2]);
        assert_eq!(headers.chunk_output_files[..2], previous_headers.chunk_output_files[..2]);
        assert!(!output.path().join("headers-0-9.tar.zst").exists());
        assert!(!output.path().join("headers-10-19.tar.zst").exists());
        for index in 2..4 {
            let path = output.path().join(headers.chunk_relative_path("headers", index));
            assert_eq!(
                reth_fs_util::metadata(path).unwrap().len(),
                headers.chunk_sizes[index as usize]
            );
        }

        // A later snapshot keeps pointing reused chunks at their original archives.
        let mut manifest = manifest;
        manifest.base_url = Some("https://example.com/current".to_string());
        let next_source = snapshot_source();
        write_test_static_files(next_source.path(), StaticFileSegment::Headers, 44);
        let next = generate_manifest(
            next_source.path(),
            tempdir().unwrap().path(),
            None,
            44,
            1,
            10,
            Some(&manifest),
        )
        .unwrap();
        assert_eq!(
            next.archive_urls(SnapshotComponentType::Headers),
            vec![
                "https://example.com/previous/headers-0-9.tar.zst",
                "https://example.com/previous/headers-10-19.tar.zst",
                "https://example.com/current/headers-20-29.tar.zst",
                "headers-30-39.tar.zst",
                "headers-40-49.tar.zst",
            ]
        );
    }

    #[test]
    fn generate_manifest_rejects_sealed_chunk_that_differs_from_previous_manifest() {
        let source = snapshot_source();
        write_test_static_files(source.path(), StaticFileSegment::Headers, 24);
        let mut previous = generate_manifest(
            source.path(),
            tempdir().unwrap().path(),
            Some("https://example.com/previous"),
            24,
            1,
            10,
            None,
        )
        .unwrap();
        let Some(ComponentManifest::Chunked(headers)) =
            previous.components.get_mut(SnapshotComponentType::Headers.key())
        else {
            panic!("headers should be chunked");
        };

        headers.chunk_output_files[0][0].blake3 = "00".repeat(32);

        let err = generate_manifest(
            source.path(),
            tempdir().unwrap().path(),
            None,
            24,
            1,
            10,
            Some(&previous),
        )
        .unwrap_err();
        assert!(err.to_string().contains("differs from the previous manifest"), "{err}");
    }

    #[test]
    fn generate_manifest_rejects_previous_manifest_without_absolute_base_url() {
        let source = snapshot_source();
        write_test_static_files(source.path(), StaticFileSegment::Headers, 24);
        let mut previous = generate_manifest(
            source.path(),
            tempdir().unwrap().path(),
            Some("https://example.com/previous"),
            24,
            1,
            10,
            None,
        )
        .unwrap();

        for base_url in [None, Some(""), Some("previous")] {
            previous.base_url = base_url.map(str::to_owned);
            let err = generate_manifest(
                source.path(),
                tempdir().unwrap().path(),
                None,
                24,
                1,
                10,
                Some(&previous),
            )
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                "Previous manifest has no absolute base_url to locate its archives"
            );
        }
    }

    #[test]
    fn generate_manifest_packages_chunks_when_previous_chunk_files_are_inconsistent() {
        let source = snapshot_source();
        write_test_static_files(source.path(), StaticFileSegment::Headers, 24);
        let mut previous = generate_manifest(
            source.path(),
            tempdir().unwrap().path(),
            Some("https://example.com/previous"),
            24,
            1,
            10,
            None,
        )
        .unwrap();
        let Some(ComponentManifest::Chunked(headers)) =
            previous.components.get_mut(SnapshotComponentType::Headers.key())
        else {
            panic!("headers should be chunked");
        };

        headers.chunk_files.truncate(1);

        let output = tempdir().unwrap();
        let manifest =
            generate_manifest(source.path(), output.path(), None, 24, 1, 10, Some(&previous))
                .unwrap();
        assert_eq!(
            manifest.archive_urls(SnapshotComponentType::Headers),
            vec!["headers-0-9.tar.zst", "headers-10-19.tar.zst", "headers-20-29.tar.zst"]
        );
        assert!(output.path().join("headers-0-9.tar.zst").exists());
    }

    #[test]
    fn generate_manifest_packages_chunks_when_previous_blocks_per_file_is_zero() {
        let source = snapshot_source();
        write_test_static_files(source.path(), StaticFileSegment::Headers, 24);
        let mut previous = generate_manifest(
            source.path(),
            tempdir().unwrap().path(),
            Some("https://example.com/previous"),
            24,
            1,
            10,
            None,
        )
        .unwrap();
        let Some(ComponentManifest::Chunked(headers)) =
            previous.components.get_mut(SnapshotComponentType::Headers.key())
        else {
            panic!("headers should be chunked");
        };

        headers.blocks_per_file = 0;

        let output = tempdir().unwrap();
        let manifest =
            generate_manifest(source.path(), output.path(), None, 24, 1, 10, Some(&previous))
                .unwrap();
        assert_eq!(
            manifest.archive_urls(SnapshotComponentType::Headers),
            vec!["headers-0-9.tar.zst", "headers-10-19.tar.zst", "headers-20-29.tar.zst"]
        );
        assert!(output.path().join("headers-0-9.tar.zst").exists());
    }
}
