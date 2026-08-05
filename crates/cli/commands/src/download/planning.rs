use super::{manifest::*, verify::OutputVerifier};
use eyre::Result;
use serde::Serialize;
use std::{collections::BTreeMap, io::Write, path::Path};
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
    /// Total compressed download size of all planned archives.
    pub(crate) total_download_size: u64,
    /// Total extracted plain-output size of all planned archives.
    pub(crate) total_output_size: u64,
}

impl PlannedDownloads {
    /// Returns the number of concrete archives queued for this snapshot selection.
    pub(crate) const fn total_archives(&self) -> usize {
        self.archives.len()
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

/// Startup summary showing how much of the selected work can be reused.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DownloadStartupSummary {
    /// Archives whose declared outputs already verify on disk.
    pub(crate) reusable: usize,
    /// Archives that still need to be downloaded or retried.
    pub(crate) needs_download: usize,
}

/// Checks selected archives against existing output files before work begins.
pub(crate) fn summarize_download_startup(
    all_downloads: &[PlannedArchive],
    target_dir: &Path,
) -> Result<DownloadStartupSummary> {
    let mut summary = DownloadStartupSummary::default();
    let verifier = OutputVerifier::new(target_dir);

    for planned in all_downloads {
        if verifier.verify(&planned.archive.output_files)? {
            summary.reusable += 1;
        } else {
            summary.needs_download += 1;
        }
    }

    Ok(summary)
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
    let mut total_download_size = 0;
    let mut total_output_size = 0;

    for (ty, selection) in selections {
        let Some(distance) = selection_archive_distance(selection, manifest.block) else {
            continue;
        };
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

            archives.push(PlannedArchive { ty: *ty, component: component.clone(), archive });
        }
    }

    sort_planned_archives(&mut archives);
    Ok(PlannedDownloads { archives, total_download_size, total_output_size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn summarize_download_startup_counts_reusable_and_needs_download() {
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

        let summary = summarize_download_startup(&planned, target_dir).unwrap();
        assert_eq!(summary.reusable, 1);
        assert_eq!(summary.needs_download, 2);
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
}
