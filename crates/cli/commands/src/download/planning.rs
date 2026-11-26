use super::{managed_datadir_path, manifest::*, verify::OutputVerifier, MANAGED_DATADIR_PATHS};
use eyre::Result;
use rayon::prelude::*;
use reth_config::config::Config;
use reth_db::static_file::{
    read_static_file_header, CONFIG_FILE_EXTENSION, OFFSETS_FILE_EXTENSION,
};
use reth_static_file_types::StaticFileSegment;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    io::Write,
    path::Path,
};
use tracing::info;

/// Machine-readable description of the archives selected for a modular snapshot download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPlan {
    /// Version of this JSON schema.
    pub schema_version: u8,
    /// Chain ID declared by the snapshot manifest.
    pub chain_id: u64,
    /// Block declared by the snapshot manifest.
    pub block: u64,
    /// Total compressed bytes for all selected archives.
    pub total_download_size: u64,
    /// Total extracted bytes for all selected archives.
    pub total_output_size: u64,
    /// Archives selected by the command's component and range flags.
    pub archives: Vec<DownloadPlanArchive>,
}

impl DownloadPlan {
    const SCHEMA_VERSION: u8 = 1;

    pub(crate) fn from_planned(manifest: &SnapshotManifest, planned: &PlannedDownloads) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            chain_id: manifest.chain_id,
            block: manifest.block,
            total_download_size: planned.total_download_size,
            total_output_size: planned.total_output_size,
            archives: planned.archives.iter().map(DownloadPlanArchive::from_planned).collect(),
        }
    }

    /// Adds an archive supplied by a command that extends the base Reth snapshot format.
    pub fn push_archive(&mut self, archive: DownloadPlanArchive) {
        self.total_download_size = self.total_download_size.saturating_add(archive.download_size);
        self.total_output_size = self.total_output_size.saturating_add(archive.output_size);
        self.archives.push(archive);
    }

    /// Writes the plan as pretty-printed JSON followed by a newline.
    pub fn write_json(&self, mut writer: impl Write) -> Result<()> {
        serde_json::to_writer_pretty(&mut writer, self)?;
        writeln!(writer)?;
        Ok(())
    }
}

/// One concrete archive in a [`DownloadPlan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPlanArchive {
    /// Stable manifest component key, such as `state` or `transactions`.
    pub component: String,
    /// Archive file name relative to the manifest's base URL.
    pub file_name: String,
    /// Resolved archive source URL.
    pub url: String,
    /// Compressed bytes fetched for this archive.
    pub download_size: u64,
    /// Extracted bytes produced by this archive.
    pub output_size: u64,
    /// Optional checksum of the compressed archive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
}

impl DownloadPlanArchive {
    /// Creates an archive entry for a download-command extension.
    pub fn new(
        component: impl Into<String>,
        file_name: impl Into<String>,
        url: impl Into<String>,
        download_size: u64,
        output_size: u64,
        blake3: Option<String>,
    ) -> Self {
        Self {
            component: component.into(),
            file_name: file_name.into(),
            url: url.into(),
            download_size,
            output_size,
            blake3,
        }
    }

    fn from_planned(planned: &PlannedArchive) -> Self {
        Self::new(
            planned.ty.key(),
            planned.archive.file_name.clone(),
            planned.archive.url.clone(),
            planned.archive.size,
            planned.archive.output_size(),
            planned.archive.blake3.clone(),
        )
    }
}

/// One archive selected from the manifest, along with its component name.
#[derive(Debug, Clone)]
pub(crate) struct PlannedArchive {
    /// Snapshot component type this archive belongs to.
    pub(crate) ty: SnapshotComponentType,
    /// User-facing component name used in logs.
    pub(crate) component: String,
    /// Concrete snapshot archive metadata resolved from the manifest.
    pub(crate) archive: SnapshotArchive,
}

/// The archive list for a modular snapshot download.
#[derive(Debug)]
pub(crate) struct PlannedDownloads {
    /// Concrete archives that still need reuse checks or processing.
    pub(crate) archives: Vec<PlannedArchive>,
    /// Validated transaction jar names selected for the restore.
    pub(super) transaction_jars: Vec<String>,
    /// Total compressed download size of all planned archives.
    pub(crate) total_download_size: u64,
    /// Total extracted plain-output size of all planned archives.
    pub(crate) total_output_size: u64,
}

impl PlannedDownloads {
    /// Checks the existing node config before cleanup unless the download replaces it.
    pub(super) fn validate_retained_config(
        &self,
        config: &Path,
        target: &Path,
        static_files: Option<&Path>,
        cleanup: bool,
    ) -> Result<()> {
        let cleared = cleanup &&
            MANAGED_DATADIR_PATHS.iter().any(|entry| {
                config.starts_with(managed_datadir_path(entry, target, static_files))
            });
        if cleared || !config.try_exists()? {
            return Ok(())
        }
        let verifier = OutputVerifier::new(target, static_files);
        for output in self.archives.iter().flat_map(|planned| &planned.archive.output_files) {
            if verifier.output_path(output)? == config {
                return Ok(())
            }
        }
        Config::from_path(config)?;
        Ok(())
    }

    /// Verifies every archive's existing outputs in parallel and splits off the archives that
    /// already match, so each output file is hashed at most once before work begins.
    pub(crate) fn partition_reusable(
        self,
        target_dir: &Path,
        static_files_dir: Option<&Path>,
    ) -> Result<CheckedDownloads> {
        let verifier = OutputVerifier::new(target_dir, static_files_dir);
        let checked = self
            .archives
            .into_par_iter()
            .map(|planned| Ok((verifier.verify(&planned.archive.output_files)?, planned)))
            .collect::<Result<Vec<_>>>()?;

        let mut downloads = CheckedDownloads {
            pending: Vec::new(),
            reused: Vec::new(),
            total_download_size: self.total_download_size,
            total_output_size: self.total_output_size,
        };
        for (reusable, planned) in checked {
            if reusable {
                downloads.reused.push(planned);
            } else {
                downloads.pending.push(planned);
            }
        }
        Ok(downloads)
    }
}

/// Planned archives split by the startup reuse check.
#[derive(Debug)]
pub(crate) struct CheckedDownloads {
    /// Archives that still need to be fetched and extracted.
    pub(crate) pending: Vec<PlannedArchive>,
    /// Archives whose declared outputs already verified on disk.
    pub(crate) reused: Vec<PlannedArchive>,
    /// Total compressed download size of all planned archives.
    pub(crate) total_download_size: u64,
    /// Total extracted plain-output size of all planned archives.
    pub(crate) total_output_size: u64,
}

impl CheckedDownloads {
    /// Returns the number of concrete archives in this snapshot selection, including reused ones.
    pub(crate) const fn total_archives(&self) -> usize {
        self.pending.len() + self.reused.len()
    }
}

/// Returns the sort priority used to schedule archives.
pub(crate) const fn archive_priority_rank(ty: SnapshotComponentType) -> u8 {
    match ty {
        SnapshotComponentType::State => 0,
        SnapshotComponentType::RocksdbIndices => 1,
        _ => 2,
    }
}

/// Converts a selection into the manifest distance form used for archive lookup.
fn selection_archive_distance(
    selection: &ComponentSelection,
    snapshot_block: u64,
) -> Option<Option<u64>> {
    match selection {
        ComponentSelection::All => Some(None),
        ComponentSelection::Distance(distance) => Some(Some(*distance)),
        ComponentSelection::Since(block) => Some(Some(snapshot_block.saturating_sub(*block) + 1)),
        ComponentSelection::None => None,
    }
}

/// Checks that every selected transaction jar has all its members, even across archives.
/// Returns the data filenames directly under the static files directory.
fn selected_transaction_jars<'a>(paths: impl Iterator<Item = &'a Path>) -> Result<Vec<String>> {
    let files: HashSet<_> = paths.map(|path| path.strip_prefix(".").unwrap_or(path)).collect();
    let mut jars = Vec::new();
    for path in &files {
        if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) &&
            let Some((StaticFileSegment::Transactions, range)) =
                StaticFileSegment::parse_filename(name) &&
            StaticFileSegment::Transactions.filename(&range) == name &&
            path.extension()
                .is_none_or(|ext| ext == CONFIG_FILE_EXTENSION || ext == OFFSETS_FILE_EXTENSION)
        {
            eyre::ensure!(
                path.parent() == Some(Path::new("static_files")),
                "Transaction static file {name} must be directly under static_files"
            );
            for extension in ["", CONFIG_FILE_EXTENSION, OFFSETS_FILE_EXTENSION] {
                let required = path.with_extension(extension);
                eyre::ensure!(
                    files.contains(required.as_path()),
                    "Incomplete transaction static file: missing {}",
                    required.display()
                );
            }
            if path.extension().is_none() {
                jars.push(name.to_owned());
            }
        }
    }
    eyre::ensure!(
        files.is_empty() || !jars.is_empty(),
        "Selected transaction archives contain no transaction static files"
    );
    Ok(jars)
}

/// Returns the last block before the continuous range of selected bodies through the tip,
/// or `None` when the range reaches genesis.
pub(super) fn restored_bodies_boundary(
    dir: &Path,
    jars: &[String],
    tip: u64,
) -> Result<Option<u64>> {
    let mut ranges = Vec::new();
    for name in jars {
        let header = read_static_file_header(&dir.join(name))?;
        if header.segment() == StaticFileSegment::Transactions &&
            let Some(range) = header.block_range()
        {
            ranges.push(range);
        }
    }
    ranges.sort_unstable_by_key(|range| std::cmp::Reverse(range.end()));
    let mut boundary = Some(tip);
    for range in ranges {
        if boundary.is_some_and(|block| range.contains(block)) {
            boundary = range.start().checked_sub(1);
        }
    }
    eyre::ensure!(
        jars.is_empty() || boundary != Some(tip),
        "Selected transaction files do not cover snapshot block {tip}"
    );
    Ok(boundary)
}

/// Sorts planned archives into a stable processing order.
fn sort_planned_archives(all_downloads: &mut [PlannedArchive]) {
    all_downloads.sort_by(|a, b| {
        archive_priority_rank(a.ty)
            .cmp(&archive_priority_rank(b.ty))
            .then_with(|| a.component.cmp(&b.component))
            .then_with(|| a.archive.file_name.cmp(&b.archive.file_name))
    });
}

/// Expands component selections into the archives that need to be processed.
pub(crate) fn collect_planned_archives(
    manifest: &SnapshotManifest,
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> Result<PlannedDownloads> {
    let mut archives = Vec::new();
    let mut transaction_jars = Vec::new();
    let mut total_download_size = 0;
    let mut total_output_size = 0;

    for (ty, selection) in selections {
        let Some(distance) = selection_archive_distance(selection, manifest.block) else {
            continue;
        };

        if let Some(ComponentManifest::Chunked(chunked)) = manifest.component(*ty) &&
            !chunked.chunk_files_are_consistent()
        {
            eyre::bail!(
                "Invalid modular manifest: {} chunk_files length ({}) does not match chunk count ({})",
                ty.key(),
                chunked.chunk_files.len(),
                chunked.num_chunks()
            );
        }

        // An empty set would download nothing while the component counts as downloaded.
        if let Some(ComponentManifest::Files(set)) = manifest.component(*ty) &&
            set.archives.is_empty()
        {
            eyre::bail!("Invalid modular manifest: {} has no archives", ty.key());
        }

        total_download_size += manifest.size_for_distance(*ty, distance);
        total_output_size += manifest.output_size_for_distance(*ty, distance);

        let snapshot_archives = manifest.snapshot_archives_for_distance(*ty, distance);
        if *ty == SnapshotComponentType::Transactions {
            transaction_jars = selected_transaction_jars(
                snapshot_archives
                    .iter()
                    .flat_map(|archive| &archive.output_files)
                    .map(|output| Path::new(&output.path)),
            )?;
        }
        let component = ty.display_name().to_string();
        if !snapshot_archives.is_empty() {
            info!(target: "reth::cli",
                component = %component,
                archives = snapshot_archives.len(),
                selection = %selection,
                "Queued component for download"
            );
        }

        for archive in snapshot_archives {
            if archive.output_files.is_empty() {
                eyre::bail!(
                    "Invalid modular manifest: {} is missing plain output checksum metadata",
                    archive.file_name
                );
            }

            archives.push(PlannedArchive { ty: *ty, component: component.clone(), archive });
        }
    }

    sort_planned_archives(&mut archives);
    Ok(PlannedDownloads { archives, transaction_jars, total_download_size, total_output_size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::tests::transaction_snapshot;
    use tempfile::tempdir;

    fn transaction_jar_outputs(start: u64, width: u64, size: u64) -> Vec<OutputFileChecksum> {
        let end = start + width - 1;
        ["", ".conf", ".off"]
            .into_iter()
            .map(|ext| OutputFileChecksum {
                path: format!("static_files/static_file_transactions_{start}_{end}{ext}"),
                size: if ext.is_empty() { size } else { 0 },
                blake3: String::new(),
            })
            .collect()
    }

    fn test_manifest_with_component_tip(block: u64, total_blocks: u64) -> SnapshotManifest {
        let chunk_count = total_blocks.div_ceil(10);
        SnapshotManifest {
            block,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
            components: BTreeMap::from([(
                "transactions".to_string(),
                ComponentManifest::Chunked(ChunkedArchive {
                    blocks_per_file: 10,
                    total_blocks,
                    chunk_sizes: (1..=chunk_count).map(|i| i * 10).collect(),
                    chunk_decompressed_sizes: (1..=chunk_count).map(|i| i * 100).collect(),
                    chunk_files: vec![],
                    chunk_output_files: (0..chunk_count)
                        .map(|i| transaction_jar_outputs(i * 10, 10, (i + 1) * 100))
                        .collect(),
                }),
            )]),
            extensions: Default::default(),
        }
    }

    #[test]
    fn partition_reusable_moves_only_verified_archives() {
        let dir = tempdir().unwrap();
        let target_dir = dir.path();
        let ok_file = target_dir.join("ok.bin");
        std::fs::write(&ok_file, vec![1_u8; 4]).unwrap();
        let ok_hash = blake3::hash(&[1_u8; 4]).to_hex().to_string();

        let planned = vec![
            PlannedArchive {
                ty: SnapshotComponentType::State,
                component: "State".to_string(),
                archive: SnapshotArchive {
                    url: "https://example.com/ok.tar.zst".to_string(),
                    file_name: "ok.tar.zst".to_string(),
                    size: 10,
                    blake3: None,
                    output_files: vec![OutputFileChecksum {
                        path: "ok.bin".to_string(),
                        size: 4,
                        blake3: ok_hash,
                    }],
                },
            },
            PlannedArchive {
                ty: SnapshotComponentType::Headers,
                component: "Headers".to_string(),
                archive: SnapshotArchive {
                    url: "https://example.com/missing.tar.zst".to_string(),
                    file_name: "missing.tar.zst".to_string(),
                    size: 10,
                    blake3: None,
                    output_files: vec![OutputFileChecksum {
                        path: "missing.bin".to_string(),
                        size: 1,
                        blake3: "deadbeef".to_string(),
                    }],
                },
            },
            PlannedArchive {
                ty: SnapshotComponentType::Transactions,
                component: "Transactions".to_string(),
                archive: SnapshotArchive {
                    url: "https://example.com/bad-size.tar.zst".to_string(),
                    file_name: "bad-size.tar.zst".to_string(),
                    size: 10,
                    blake3: None,
                    output_files: vec![],
                },
            },
        ];

        let downloads = PlannedDownloads {
            archives: planned,
            transaction_jars: vec![],
            total_download_size: 30,
            total_output_size: 5,
        }
        .partition_reusable(target_dir, None)
        .unwrap();
        assert_eq!(downloads.total_archives(), 3);

        let names = |archives: &[PlannedArchive]| {
            archives.iter().map(|planned| planned.archive.file_name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(&downloads.reused), ["ok.tar.zst"]);
        assert_eq!(names(&downloads.pending), ["missing.tar.zst", "bad-size.tar.zst"]);
    }

    #[test]
    fn rocksdb_archive_set_reuses_each_verified_sst() {
        let dir = tempdir().unwrap();
        let rocksdb_dir = dir.path().join("rocksdb");
        std::fs::create_dir_all(&rocksdb_dir).unwrap();
        std::fs::write(rocksdb_dir.join("000007.sst"), b"sst-7").unwrap();

        let sst_archive = |number: u32, contents: &[u8]| {
            let hash = blake3::hash(contents).to_hex().to_string();
            SingleArchive {
                file: format!("rocksdb_indices/{number:06}-{hash}.tar.zst"),
                size: 10,
                decompressed_size: contents.len() as u64,
                blake3: None,
                output_files: vec![OutputFileChecksum {
                    path: format!("rocksdb/{number:06}.sst"),
                    size: contents.len() as u64,
                    blake3: hash,
                }],
            }
        };
        let manifest = SnapshotManifest {
            block: 1,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
            components: BTreeMap::from([(
                SnapshotComponentType::RocksdbIndices.key().to_string(),
                ComponentManifest::Files(ArchiveSet {
                    archives: vec![sst_archive(7, b"sst-7"), sst_archive(9, b"sst-9")],
                }),
            )]),
            extensions: Default::default(),
        };

        let selections =
            BTreeMap::from([(SnapshotComponentType::RocksdbIndices, ComponentSelection::All)]);
        let planned = collect_planned_archives(&manifest, &selections).unwrap();
        assert_eq!(planned.total_download_size, 20);
        assert_eq!(planned.total_output_size, 10);

        let downloads = planned.partition_reusable(dir.path(), None).unwrap();
        let names = |archives: &[PlannedArchive]| {
            archives.iter().map(|planned| planned.archive.file_name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(&downloads.reused), [sst_archive(7, b"sst-7").file]);
        assert_eq!(names(&downloads.pending), [sst_archive(9, b"sst-9").file]);
    }

    #[test]
    fn archive_priority_prefers_state_then_rocksdb() {
        let mut planned = [
            PlannedArchive {
                ty: SnapshotComponentType::Transactions,
                component: "Transactions".to_string(),
                archive: SnapshotArchive {
                    url: "u3".to_string(),
                    file_name: "t.tar.zst".to_string(),
                    size: 1,
                    blake3: None,
                    output_files: vec![OutputFileChecksum {
                        path: "a".to_string(),
                        size: 1,
                        blake3: "x".to_string(),
                    }],
                },
            },
            PlannedArchive {
                ty: SnapshotComponentType::RocksdbIndices,
                component: "RocksDB Indices".to_string(),
                archive: SnapshotArchive {
                    url: "u2".to_string(),
                    file_name: "rocksdb_indices.tar.zst".to_string(),
                    size: 1,
                    blake3: None,
                    output_files: vec![OutputFileChecksum {
                        path: "b".to_string(),
                        size: 1,
                        blake3: "y".to_string(),
                    }],
                },
            },
            PlannedArchive {
                ty: SnapshotComponentType::State,
                component: "State (mdbx)".to_string(),
                archive: SnapshotArchive {
                    url: "u1".to_string(),
                    file_name: "state.tar.zst".to_string(),
                    size: 1,
                    blake3: None,
                    output_files: vec![OutputFileChecksum {
                        path: "c".to_string(),
                        size: 1,
                        blake3: "z".to_string(),
                    }],
                },
            },
        ];

        planned.sort_by(|a, b| {
            archive_priority_rank(a.ty)
                .cmp(&archive_priority_rank(b.ty))
                .then_with(|| a.component.cmp(&b.component))
                .then_with(|| a.archive.file_name.cmp(&b.archive.file_name))
        });

        assert_eq!(planned[0].ty, SnapshotComponentType::State);
        assert_eq!(planned[1].ty, SnapshotComponentType::RocksdbIndices);
        assert_eq!(planned[2].ty, SnapshotComponentType::Transactions);
    }

    #[test]
    fn collect_planned_archives_tracks_download_and_output_totals() {
        let mut components = BTreeMap::new();
        components.insert(
            "state".to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "state.tar.zst".to_string(),
                size: 10,
                decompressed_size: 100,
                blake3: None,
                output_files: vec![OutputFileChecksum {
                    path: "db/mdbx.dat".to_string(),
                    size: 100,
                    blake3: "h0".to_string(),
                }],
            }),
        );
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_000_000,
                chunk_sizes: vec![20, 30],
                chunk_decompressed_sizes: vec![200, 300],
                chunk_files: vec![],
                chunk_output_files: vec![
                    transaction_jar_outputs(0, 500_000, 200),
                    transaction_jar_outputs(500_000, 500_000, 300),
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

        let selections = BTreeMap::from([
            (SnapshotComponentType::State, ComponentSelection::All),
            (SnapshotComponentType::Transactions, ComponentSelection::Distance(500_000)),
        ]);

        let planned = collect_planned_archives(&manifest, &selections).unwrap();

        assert_eq!(planned.total_download_size, 40);
        assert_eq!(planned.total_output_size, 400);
        assert_eq!(planned.archives.len(), 2);

        let plan = DownloadPlan::from_planned(&manifest, &planned);
        assert_eq!(
            serde_json::to_value(plan).unwrap(),
            serde_json::json!({
                "schemaVersion": 1,
                "chainId": 1,
                "block": 1_000_000,
                "totalDownloadSize": 40,
                "totalOutputSize": 400,
                "archives": [
                    {
                        "component": "state",
                        "fileName": "state.tar.zst",
                        "url": "https://example.com/state.tar.zst",
                        "downloadSize": 10,
                        "outputSize": 100
                    },
                    {
                        "component": "transactions",
                        "fileName": "transactions-500000-999999.tar.zst",
                        "url": "https://example.com/transactions-500000-999999.tar.zst",
                        "downloadSize": 30,
                        "outputSize": 300
                    }
                ]
            })
        );
    }

    #[test]
    fn collect_planned_archives_rejects_empty_archive_set() {
        let manifest = SnapshotManifest {
            block: 1,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
            components: BTreeMap::from([(
                SnapshotComponentType::RocksdbIndices.key().to_string(),
                ComponentManifest::Files(ArchiveSet { archives: vec![] }),
            )]),
            extensions: Default::default(),
        };
        let selections =
            BTreeMap::from([(SnapshotComponentType::RocksdbIndices, ComponentSelection::All)]);

        let err = collect_planned_archives(&manifest, &selections).unwrap_err();
        assert_eq!(err.to_string(), "Invalid modular manifest: rocksdb_indices has no archives");
    }

    #[test]
    fn collect_planned_archives_rejects_mismatched_chunk_files_length() {
        let mut components = BTreeMap::new();
        components.insert(
            "transactions".to_string(),
            ComponentManifest::Chunked(ChunkedArchive {
                blocks_per_file: 500_000,
                total_blocks: 1_000_000,
                chunk_sizes: vec![20, 30],
                chunk_decompressed_sizes: vec![200, 300],
                chunk_files: vec!["static_files/transactions-0-499999.tar.zst".to_string()],
                chunk_output_files: vec![
                    vec![OutputFileChecksum {
                        path: "static_files/tx-0".to_string(),
                        size: 200,
                        blake3: "h1".to_string(),
                    }],
                    vec![OutputFileChecksum {
                        path: "static_files/tx-1".to_string(),
                        size: 300,
                        blake3: "h2".to_string(),
                    }],
                ],
            }),
        );

        let manifest = SnapshotManifest {
            block: 1_000_000,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com/mainnet".to_string()),
            reth_version: None,
            components,
            extensions: Default::default(),
        };
        let selections =
            BTreeMap::from([(SnapshotComponentType::Transactions, ComponentSelection::All)]);

        let err = collect_planned_archives(&manifest, &selections).unwrap_err();
        assert!(
            err.to_string().contains("chunk_files length"),
            "expected chunk_files length error, got: {err}"
        );
    }

    #[test]
    fn range_selections_use_snapshot_block_when_component_extends_past_it() {
        // The snapshot ends at 20, but the final archive contains blocks 20 through 24.
        let manifest = test_manifest_with_component_tip(20, 25);
        let component = SnapshotComponentType::Transactions;
        for (selection, expected_chunks) in [
            (ComponentSelection::Since(19), vec![1, 2]),
            (ComponentSelection::Distance(2), vec![1, 2]),
            (ComponentSelection::Since(20), vec![2]),
            (ComponentSelection::Distance(1), vec![2]),
            (ComponentSelection::Since(10), vec![1, 2]),
            (ComponentSelection::Since(9), vec![0, 1, 2]),
            (ComponentSelection::Distance(0), vec![]),
            (ComponentSelection::Distance(u64::MAX), vec![0, 1, 2]),
            (ComponentSelection::All, vec![0, 1, 2]),
        ] {
            let planned =
                collect_planned_archives(&manifest, &BTreeMap::from([(component, selection)]))
                    .unwrap();
            let expected_files = expected_chunks
                .iter()
                .map(|i| format!("transactions-{}-{}.tar.zst", i * 10, (i + 1) * 10 - 1))
                .collect::<Vec<_>>();
            let expected_size = expected_chunks.iter().map(|i| (i + 1) * 10).sum::<u64>();
            assert_eq!(
                planned.archives.iter().map(|a| a.archive.file_name.clone()).collect::<Vec<_>>(),
                expected_files,
                "{selection}"
            );
            assert_eq!(planned.total_download_size, expected_size, "{selection}");
            assert_eq!(planned.total_output_size, expected_size * 10, "{selection}");

            // The URL, count, and size APIs used by the UI must agree with the download plan.
            let distance = selection_archive_distance(&selection, manifest.block).unwrap();
            assert_eq!(
                manifest.archive_urls_for_distance(component, distance),
                expected_files
                    .iter()
                    .map(|file| format!("https://example.com/{file}"))
                    .collect::<Vec<_>>(),
                "{selection}"
            );
            assert_eq!(
                manifest.chunks_for_distance(component, distance),
                expected_chunks.len() as u64,
                "{selection}"
            );
            assert_eq!(
                manifest.size_for_distance(component, distance),
                expected_size,
                "{selection}"
            );
            assert_eq!(
                manifest.output_size_for_distance(component, distance),
                expected_size * 10,
                "{selection}"
            );
        }
    }

    #[test]
    fn range_selections_do_not_shift_back_to_a_shorter_component_tip() {
        let manifest = test_manifest_with_component_tip(20, 18);
        let component = SnapshotComponentType::Transactions;
        for (selection, expected_files) in [
            (ComponentSelection::Since(19), vec![]),
            (ComponentSelection::Distance(2), vec![]),
            (ComponentSelection::Since(17), vec!["transactions-10-19.tar.zst"]),
            (ComponentSelection::Distance(4), vec!["transactions-10-19.tar.zst"]),
        ] {
            let planned =
                collect_planned_archives(&manifest, &BTreeMap::from([(component, selection)]))
                    .unwrap();
            assert_eq!(
                planned.archives.iter().map(|a| a.archive.file_name.as_str()).collect::<Vec<_>>(),
                expected_files,
                "{selection}"
            );
            let expected_size = if expected_files.is_empty() { 0 } else { 20 };
            assert_eq!(planned.total_download_size, expected_size, "{selection}");
            assert_eq!(planned.total_output_size, expected_size * 10, "{selection}");
        }
    }

    #[test]
    fn range_selections_preserve_legacy_tip_archives() {
        // Legacy manifests stored the snapshot height in total_blocks, not the block count.
        // At an exact chunk boundary, the legacy generator did not emit the next archive.
        for (block, expected_files, expected_size) in
            [(24, vec!["transactions-20-29.tar.zst"], 30), (20, vec![], 0)]
        {
            let manifest = test_manifest_with_component_tip(block, block);
            let component = SnapshotComponentType::Transactions;
            for selection in [ComponentSelection::Since(block), ComponentSelection::Distance(1)] {
                let planned =
                    collect_planned_archives(&manifest, &BTreeMap::from([(component, selection)]))
                        .unwrap();
                assert_eq!(
                    planned
                        .archives
                        .iter()
                        .map(|a| a.archive.file_name.as_str())
                        .collect::<Vec<_>>(),
                    expected_files,
                    "{selection}"
                );
                assert_eq!(planned.total_download_size, expected_size, "{selection}");
                assert_eq!(planned.total_output_size, expected_size * 10, "{selection}");
                assert_eq!(
                    manifest.chunks_for_distance(component, Some(1)),
                    expected_files.len() as u64,
                    "{selection}"
                );
            }
        }
    }

    #[test]
    fn transaction_jar_outside_static_files_root_is_rejected() {
        for directory in ["static_files/backup", "backup", ""] {
            let path = Path::new(directory).join("static_file_transactions_0_9");
            let members = ["", CONFIG_FILE_EXTENSION, OFFSETS_FILE_EXTENSION]
                .map(|ext| path.with_extension(ext));
            let err =
                selected_transaction_jars(members.iter().map(|path| path.as_path())).unwrap_err();
            assert_eq!(err.to_string(), "Transaction static file static_file_transactions_0_9 must be directly under static_files");
        }
    }

    #[test]
    fn transaction_archives_with_ambiguous_coverage() -> Result<()> {
        let archives = tempdir()?;
        let txs = SnapshotComponentType::Transactions;
        for actual_tip in [24, 25] {
            let files = transaction_snapshot(actual_tip)?;
            let mut manifest =
                generate_manifest(files.path(), archives.path(), None, 25, 1, 10, None, false)?;
            let Some(ComponentManifest::Chunked(chunked)) = manifest.components.get_mut(txs.key())
            else {
                panic!("missing transaction chunks")
            };
            chunked.total_blocks = 25;
            // A non-chunked archive can omit the middle file even if it exists on disk.
            let archive = SingleArchive {
                file: "transactions.tar.zst".into(),
                size: 0,
                decompressed_size: 0,
                blake3: None,
                output_files: [0, 2]
                    .into_iter()
                    .flat_map(|i| chunked.chunk_output_files[i].clone())
                    .collect(),
            };
            let selections = BTreeMap::from([(txs, ComponentSelection::Since(13))]);
            let check = |manifest: &SnapshotManifest, expected| -> Result<()> {
                let jars = collect_planned_archives(manifest, &selections)?.transaction_jars;
                let boundary = restored_bodies_boundary(
                    &files.path().join("static_files"),
                    &jars,
                    manifest.block,
                );
                if actual_tip == 25 {
                    assert_eq!(boundary?, Some(expected));
                } else {
                    assert_eq!(
                        boundary.unwrap_err().to_string(),
                        "Selected transaction files do not cover snapshot block 25"
                    );
                }
                Ok(())
            };
            check(&manifest, 9)?;
            for component in [
                ComponentManifest::Single(archive.clone()),
                ComponentManifest::Files(ArchiveSet { archives: vec![archive] }),
            ] {
                manifest.components.insert(txs.key().to_owned(), component);
                check(&manifest, 19)?;
            }
        }
        Ok(())
    }

    #[test]
    fn transaction_plan_rejects_receipt_files() {
        let ty = SnapshotComponentType::Transactions;
        let mut manifest = test_manifest_with_component_tip(9, 10);
        let Some(ComponentManifest::Chunked(chunked)) = manifest.components.get_mut(ty.key())
        else {
            unreachable!()
        };
        for output in &mut chunked.chunk_output_files[0] {
            output.path = output.path.replace("transactions", "receipts");
        }
        let err =
            collect_planned_archives(&manifest, &BTreeMap::from([(ty, ComponentSelection::All)]))
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Selected transaction archives contain no transaction static files"
        );
    }
}
