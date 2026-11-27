use super::{
    is_managed_snapshot_path, managed_datadir_path, manifest::*, verify::OutputVerifier,
    MANAGED_DATADIR_PATHS,
};
use eyre::Result;
use rayon::prelude::*;
use reth_config::config::Config;
use reth_db::static_file::{
    last_missing_block, read_static_file_header, CONFIG_FILE_EXTENSION, OFFSETS_FILE_EXTENSION,
};
use reth_static_file_types::StaticFileSegment;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
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
            planned.archive.size.unwrap_or_default(),
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
    /// Total compressed download size of all planned archives.
    pub(crate) total_download_size: u64,
    /// Total extracted plain-output size of all planned archives.
    pub(crate) total_output_size: u64,
}

impl PlannedDownloads {
    /// Collects transaction jars from the selected archives after all manifest formats have
    /// been expanded. A jar's members can be distributed across archives.
    pub(super) fn transaction_jars(&self) -> Result<Vec<String>> {
        let files: HashSet<_> = self
            .archives
            .iter()
            .filter(|planned| planned.ty == SnapshotComponentType::Transactions)
            .flat_map(|planned| &planned.archive.output_files)
            .map(|output| {
                let path = Path::new(&output.path);
                path.strip_prefix(".").unwrap_or(path)
            })
            .collect();
        let mut jars = HashSet::new();
        for path in &files {
            let extension = path.extension().and_then(|ext| ext.to_str());
            if matches!(extension, None | Some(CONFIG_FILE_EXTENSION | OFFSETS_FILE_EXTENSION)) &&
                let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) &&
                let Some((StaticFileSegment::Transactions, range)) =
                    StaticFileSegment::parse_filename(name) &&
                StaticFileSegment::Transactions.filename(&range) == name
            {
                eyre::ensure!(
                    path.parent() == Some(Path::new("static_files")),
                    "Transaction static file {name} must be directly under static_files"
                );
                if !jars.insert(name) {
                    continue
                }
                for extension in ["", CONFIG_FILE_EXTENSION, OFFSETS_FILE_EXTENSION] {
                    let required = path.with_extension(extension);
                    eyre::ensure!(
                        files.contains(required.as_path()),
                        "Incomplete transaction static file: missing {}",
                        required.display()
                    );
                }
            }
        }
        eyre::ensure!(
            files.is_empty() || !jars.is_empty(),
            "Selected transaction archives contain no transaction static files"
        );
        Ok(jars.into_iter().map(str::to_owned).collect())
    }

    /// Resolves parent traversal before cleanup can remove an intermediate directory and
    /// checks the node configuration if the download retains it.
    pub(super) fn validated_config_path(
        &self,
        config: PathBuf,
        target: &Path,
        static_files: Option<&Path>,
        cleanup: bool,
    ) -> Result<PathBuf> {
        let parent = config.ancestors().find(|path| path.ends_with(".."));
        let traversed = parent.is_some();
        // Resolve only through the final `..`. The remaining path may not exist yet.
        let resolved = if let Some(parent) = parent {
            parent.canonicalize()?.join(config.strip_prefix(parent)?)
        } else {
            config
        };
        if !resolved.try_exists()? {
            return Ok(resolved)
        }
        if cleanup {
            for entry in MANAGED_DATADIR_PATHS {
                let root = managed_datadir_path(entry, target, static_files);
                if resolved.starts_with(&root) ||
                    (traversed &&
                        root.try_exists()? &&
                        !root.is_symlink() &&
                        resolved.starts_with(root.canonicalize()?))
                {
                    return Ok(resolved)
                }
            }
        }
        let verifier = OutputVerifier::new(target, static_files);
        for output in self.archives.iter().flat_map(|planned| &planned.archive.output_files) {
            let output = verifier.output_path(output)?;
            if output == resolved {
                return Ok(resolved)
            }
            if traversed &&
                output.file_name() == resolved.file_name() &&
                let (Some(output_dir), Some(config_dir)) = (output.parent(), resolved.parent()) &&
                output_dir.try_exists()? &&
                output_dir.canonicalize()? == config_dir.canonicalize()?
            {
                return Ok(resolved)
            }
        }
        Config::from_path(&resolved)?.static_files.validate()?;
        Ok(resolved)
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
    let boundary = last_missing_block(tip, ranges.into_iter());
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

/// Expands component selections into archives and validates their output paths.
pub(crate) fn collect_planned_archives(
    manifest: &SnapshotManifest,
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> Result<PlannedDownloads> {
    let mut archives = Vec::new();
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

            for output in &archive.output_files {
                let path = output.relative_path()?;
                eyre::ensure!(
                    is_managed_snapshot_path(path, false),
                    "Not a snapshot output path: `{}`",
                    path.display()
                );
            }

            archives.push(PlannedArchive { ty: *ty, component: component.clone(), archive });
        }
    }

    sort_planned_archives(&mut archives);
    Ok(PlannedDownloads { archives, total_download_size, total_output_size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::tests::snapshot_source;
    use tempfile::tempdir;

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
                        .map(|i| {
                            vec![OutputFileChecksum {
                                path: format!("static_files/tx-{i}"),
                                size: (i + 1) * 100,
                                blake3: format!("h{i}"),
                            }]
                        })
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
                    size: Some(10),
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
                    size: Some(10),
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
                    size: Some(10),
                    blake3: None,
                    output_files: vec![],
                },
            },
        ];

        let downloads =
            PlannedDownloads { archives: planned, total_download_size: 30, total_output_size: 5 }
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
                    size: Some(1),
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
                    size: Some(1),
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
                    size: Some(1),
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
    fn collect_planned_archives_rejects_a_directory_listed_as_a_file() {
        let mut manifest = test_manifest_with_component_tip(9, 10);
        let component = SnapshotComponentType::Transactions;
        let ComponentManifest::Chunked(archive) =
            manifest.components.get_mut(component.key()).unwrap()
        else {
            panic!("expected a chunked archive");
        };
        archive.chunk_output_files[0][0].path = "static_files".to_string();

        let err = collect_planned_archives(
            &manifest,
            &BTreeMap::from([(component, ComponentSelection::All)]),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "Not a snapshot output path: `static_files`");
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
    fn transaction_archives_with_ambiguous_coverage() -> Result<()> {
        let archives = tempdir()?;
        let txs = SnapshotComponentType::Transactions;
        let files = snapshot_source(Some(0..=25))?;
        let dir = files.path().join("static_files");
        let mut manifest =
            generate_manifest(files.path(), archives.path(), None, 25, 1, 10, None, false)?;
        let Some(ComponentManifest::Chunked(mut chunks)) = manifest.components.remove(txs.key())
        else {
            panic!("missing transaction chunks")
        };
        chunks.total_blocks = 25;
        // The non-chunked selections omit the middle file that is still on disk.
        let [first, _, last] = chunks.chunk_output_files.as_slice() else {
            panic!("expected three transaction chunks")
        };
        let archive = SingleArchive {
            file: "transactions.tar.zst".into(),
            size: 0,
            decompressed_size: 0,
            blake3: None,
            output_files: [first.as_slice(), last].concat(),
        };
        let selections = BTreeMap::from([(txs, ComponentSelection::Since(13))]);
        for (component, expected) in [
            (ComponentManifest::Chunked(chunks), 9),
            (ComponentManifest::Single(archive.clone()), 19),
            (ComponentManifest::Files(ArchiveSet { archives: vec![archive] }), 19),
        ] {
            manifest.components.insert(txs.key().into(), component);
            let jars = collect_planned_archives(&manifest, &selections)?.transaction_jars()?;
            assert_eq!(restored_bodies_boundary(&dir, &jars, manifest.block)?, Some(expected));
        }
        let jars = collect_planned_archives(&manifest, &selections)?.transaction_jars()?;
        let err = restored_bodies_boundary(&dir, &jars, 26).unwrap_err();
        assert_eq!(err.to_string(), "Selected transaction files do not cover snapshot block 26");
        Ok(())
    }

    #[test]
    fn transaction_plan_rejects_invalid_output_paths() -> Result<()> {
        let manifest = test_manifest_with_component_tip(9, 10);
        let selections =
            BTreeMap::from([(SnapshotComponentType::Transactions, ComponentSelection::All)]);
        let mut plan = collect_planned_archives(&manifest, &selections)?;
        let misplaced = "Transaction static file static_file_transactions_0_9 must be directly under static_files";
        for (path, expected) in [
            ("static_files/backup/static_file_transactions_0_9", misplaced),
            ("static_file_transactions_0_9", misplaced),
            (
                "static_files/static_file_receipts_0_9",
                "Selected transaction archives contain no transaction static files",
            ),
        ] {
            plan.archives[0].archive.output_files[0].path = path.into();
            assert_eq!(plan.transaction_jars().unwrap_err().to_string(), expected, "{path}");
        }
        Ok(())
    }
}
