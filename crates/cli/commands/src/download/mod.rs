//! Snapshot download command.
//!
//! `reth download` prepares a data directory from published snapshot archives. [`DownloadCommand`]
//! covers both a single-archive path and a manifest-driven path, and owns the steps required to
//! turn downloaded bytes into a bootable node directory.
//!
//! ## Entry modes
//!
//! [`DownloadCommand`] has two main execution modes:
//!
//! - Single-archive mode processes one `.tar.lz4` or `.tar.zst` archive from `--url`.
//!   Depending on the source and flags, it either extracts a local `file://` archive, streams a
//!   remote archive straight into extraction, or downloads the archive to disk first and then
//!   extracts it.
//! - Manifest mode resolves a [`SnapshotManifest`], turns CLI or TUI choices into
//!   [`ComponentSelection`]s, plans the required archives, processes them, and then writes the
//!   resulting config and database checkpoints.
//!
//! [`DownloadDefaults`] defines the discovery endpoints and default help text used when the command
//! needs to discover a manifest instead of consuming an explicit source.
//!
//! ## Selection and planning
//!
//! Manifest mode first reduces user input into `ResolvedComponents`: a map of
//! [`SnapshotComponentType`] to [`ComponentSelection`] plus an optional `SelectionPreset`.
//! This turns CLI input (`minimal`, `full`, `archive`, or explicit `--with-*` flags) into the
//! component selections used by the download code.
//!
//! The selected components are then expanded into `PlannedDownloads`, which is the set of
//! `PlannedArchive`s that must be verified, downloaded, or reused. Planning also computes the
//! total byte count used by progress reporting.
//!
//! ## Archive processing
//!
//! Each planned archive is processed independently, but `DownloadSession` holds the shared
//! progress, request limit, and cancellation token for the whole command.
//! `ArchiveProcessContext` adds the paths needed to process one archive.
//!
//! Archive processing is modeled around `ModularDownloadJob`, which schedules work, and
//! `ArchiveProcessor`, which owns the explicit retry state machine for one archive.
//! `ArchiveMode` decides whether that archive should be fetched through the cache or streamed
//! directly:
//!
//! - reuse verified plain output files when possible,
//! - otherwise fetch and extract the archive,
//! - verify the declared output files,
//! - retry the entire archive attempt if extraction succeeded but verification failed.
//!
//! Reuse and completion are based on verified output files, not on whether an old archive file is
//! present.
//!
//! ## Fetch and extraction
//!
//! `stream_and_extract` handles the single-archive path. It supports local files, resumable
//! downloads to disk, and direct streaming extraction.
//!
//! When the code needs to fetch an archive to disk, it uses `ArchiveFetcher`. The fetcher probes
//! the remote source and chooses between a sequential download and a segmented download plan
//! (`SegmentedDownloadPlan`). `SequentialDownloadFallback` records why a source could not use the
//! segmented path, while `SegmentedDownload` runs the worker queue and piece retries for the
//! parallel path.
//!
//! Segmented download retries individual byte ranges. Archive processing retries whole-archive
//! attempts. These are separate layers: range retries deal with transient request failures, while
//! archive retries deal with extraction or output verification failures.
//!
//! `CompressionFormat` determines how the archive stream is unpacked once bytes are available, and
//! `OutputVerifier` checks the extracted output files before reuse or completion.
//!
//! ## Progress and finalization
//!
//! `DownloadProgress` reports progress for the single-archive path. `SharedProgress` reports
//! aggregate progress for modular downloads. It tracks fetched bytes separately from completed
//! bytes so repeated fetches during retries do not overstate completion.
//!
//! After all required archives are complete, [`DownloadCommand`] finalizes the directory by
//! writing the derived node configuration and updating prune or index-stage checkpoints. A
//! successful command leaves a data directory that matches the snapshot shape that was selected.

mod archive;
pub mod config_gen;
mod extract;
mod fetch;
pub mod manifest;
pub mod manifest_cmd;
mod planning;
mod progress;
mod session;
mod source;
mod tui;
mod verify;

use crate::common::EnvironmentArgs;
use archive::run_modular_downloads;
use clap::{builder::RangedU64ValueParser, Parser};
use config_gen::{config_for_selections, write_config};
use extract::stream_and_extract;
use eyre::Result;
use manifest::{ComponentSelection, SnapshotComponentType, SnapshotManifest};
use planning::{collect_planned_archives, summarize_download_startup};
use progress::{DownloadProgress, DownloadRequestLimiter};
use reth_chainspec::{EthChainSpec, EthereumHardfork, EthereumHardforks, MAINNET};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_util::cancellation::CancellationToken;
use reth_db::{init_db, Database};
use reth_db_api::transaction::DbTx;
use reth_fs_util as fs;
use reth_node_core::args::DefaultPruningValues;
use reth_prune_types::PruneMode;
use source::{
    discover_manifest_url, fetch_manifest_from_source, fetch_snapshot_api_entries,
    print_snapshot_listing, resolve_manifest_base_url,
};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tracing::info;
use tui::{run_selector, SelectorOutput};

const RETH_SNAPSHOTS_BASE_URL: &str = "https://snapshots-r2.reth.rs";
const RETH_SNAPSHOTS_API_URL: &str = "https://snapshots.reth.rs/api/snapshots";

/// Maximum number of simultaneous HTTP downloads across the entire snapshot job.
const MAX_CONCURRENT_DOWNLOADS: usize = 8;

/// Built-in component presets for snapshot selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionPreset {
    /// Minimal node data needed to start from a snapshot.
    Minimal,
    /// Full-node data matching the default full prune settings.
    Full,
    /// All available snapshot data.
    Archive,
}

struct ResolvedComponents {
    selections: BTreeMap<SnapshotComponentType, ComponentSelection>,
    preset: Option<SelectionPreset>,
}

/// Global static download defaults
static DOWNLOAD_DEFAULTS: OnceLock<DownloadDefaults> = OnceLock::new();

/// Download configuration defaults
///
/// Global defaults can be set via [`DownloadDefaults::try_init`].
#[derive(Debug, Clone)]
pub struct DownloadDefaults {
    /// List of available snapshot sources
    pub available_snapshots: Vec<Cow<'static, str>>,
    /// Default base URL for snapshots
    pub default_base_url: Cow<'static, str>,
    /// Default base URL for chain-aware snapshots.
    ///
    /// When set, the chain ID is appended to form the full URL: `{base_url}/{chain_id}`.
    /// For example, given a base URL of `https://snapshots.example.com` and chain ID `1`,
    /// the resulting URL would be `https://snapshots.example.com/1`.
    ///
    /// Falls back to [`default_base_url`](Self::default_base_url) when `None`.
    pub default_chain_aware_base_url: Option<Cow<'static, str>>,
    /// URL for the snapshot discovery API that lists available snapshots.
    ///
    /// Defaults to `https://snapshots.reth.rs/api/snapshots`.
    pub snapshot_api_url: Cow<'static, str>,
    /// Optional custom long help text that overrides the generated help
    pub long_help: Option<String>,
}

impl DownloadDefaults {
    /// Initialize the global download defaults with this configuration
    pub fn try_init(self) -> Result<(), Self> {
        DOWNLOAD_DEFAULTS.set(self)
    }

    /// Get a reference to the global download defaults
    pub fn get_global() -> &'static DownloadDefaults {
        DOWNLOAD_DEFAULTS.get_or_init(DownloadDefaults::default_download_defaults)
    }

    /// Default download configuration with defaults from snapshots.reth.rs and publicnode
    pub fn default_download_defaults() -> Self {
        Self {
            available_snapshots: vec![
                Cow::Borrowed("https://snapshots.reth.rs (default)"),
                Cow::Borrowed("https://publicnode.com/snapshots (full nodes & testnets)"),
            ],
            default_base_url: Cow::Borrowed(RETH_SNAPSHOTS_BASE_URL),
            default_chain_aware_base_url: None,
            snapshot_api_url: Cow::Borrowed(RETH_SNAPSHOTS_API_URL),
            long_help: None,
        }
    }

    /// Generates the long help text for the download URL argument using these defaults.
    ///
    /// If a custom long_help is set, it will be returned. Otherwise, help text is generated
    /// from the available_snapshots list.
    pub fn long_help(&self) -> String {
        if let Some(ref custom_help) = self.long_help {
            return custom_help.clone();
        }

        let implicit_download_help = if self.mainnet_only_discovery() {
            "\nIf no URL is provided, the latest archive snapshot will only be proposed\nfor Ethereum mainnet. For other chains, provide --manifest-url, --manifest-path,\nor -u explicitly."
        } else {
            "\nIf no URL is provided, the latest archive snapshot for the selected chain\nwill be proposed for download from "
        };

        let mut help = format!(
            "Specify a snapshot URL or let the command propose a default one.\n\n\
             Browse available snapshots at {}\n\
             or use --list-snapshots to see them from the CLI.\n\nAvailable snapshot sources:\n",
            self.snapshot_api_url.trim_end_matches("/api/snapshots"),
        );

        for source in &self.available_snapshots {
            help.push_str("- ");
            help.push_str(source);
            help.push('\n');
        }

        help.push_str(implicit_download_help);
        if !self.mainnet_only_discovery() {
            help.push_str(
                self.default_chain_aware_base_url.as_deref().unwrap_or(&self.default_base_url),
            );
            help.push('.');
        }
        help.push_str(
            "\n\nLocal file:// URLs are also supported for extracting snapshots from disk.",
        );
        help
    }

    fn mainnet_only_discovery(&self) -> bool {
        self.snapshot_api_url == RETH_SNAPSHOTS_API_URL
    }

    /// Add a snapshot source to the list
    pub fn with_snapshot(mut self, source: impl Into<Cow<'static, str>>) -> Self {
        self.available_snapshots.push(source.into());
        self
    }

    /// Replace all snapshot sources
    pub fn with_snapshots(mut self, sources: Vec<Cow<'static, str>>) -> Self {
        self.available_snapshots = sources;
        self
    }

    /// Set the default base URL, e.g. `https://downloads.merkle.io`.
    pub fn with_base_url(mut self, url: impl Into<Cow<'static, str>>) -> Self {
        self.default_base_url = url.into();
        self
    }

    /// Set the default chain-aware base URL.
    pub fn with_chain_aware_base_url(mut self, url: impl Into<Cow<'static, str>>) -> Self {
        self.default_chain_aware_base_url = Some(url.into());
        self
    }

    /// Set the snapshot discovery API URL.
    pub fn with_snapshot_api_url(mut self, url: impl Into<Cow<'static, str>>) -> Self {
        self.snapshot_api_url = url.into();
        self
    }

    /// Builder: Set custom long help text, overriding the generated help
    pub fn with_long_help(mut self, help: impl Into<String>) -> Self {
        self.long_help = Some(help.into());
        self
    }
}

impl Default for DownloadDefaults {
    /// Returns the built-in download defaults.
    fn default() -> Self {
        Self::default_download_defaults()
    }
}

/// CLI command that downloads snapshot archives and configures a reth node from them.
#[derive(Debug, Parser)]
pub struct DownloadCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Custom URL to download a single snapshot archive (legacy mode).
    ///
    /// When provided, downloads and extracts a single archive without component selection.
    /// Browse available snapshots with --list-snapshots.
    #[arg(long, short, long_help = DownloadDefaults::get_global().long_help())]
    url: Option<String>,

    /// URL to a snapshot manifest.json for modular component downloads.
    ///
    /// When provided, fetches this manifest instead of discovering it from the default
    /// base URL. Useful for testing with custom or local manifests.
    #[arg(long, value_name = "URL", conflicts_with = "url")]
    manifest_url: Option<String>,

    /// Local path to a snapshot manifest.json for modular component downloads.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["url", "manifest_url"])]
    manifest_path: Option<PathBuf>,

    /// Include all transaction static files.
    #[arg(long, conflicts_with_all = ["with_txs_since", "with_txs_distance", "minimal", "full", "archive"])]
    with_txs: bool,

    /// Include transaction static files starting at the specified block.
    #[arg(long, value_name = "BLOCK_NUMBER", conflicts_with_all = ["with_txs", "with_txs_distance", "minimal", "full", "archive"])]
    with_txs_since: Option<u64>,

    /// Include transaction static files covering the last N blocks.
    #[arg(long, value_name = "BLOCKS", value_parser = RangedU64ValueParser::<u64>::new().range(1..), conflicts_with_all = ["with_txs", "with_txs_since", "minimal", "full", "archive"])]
    with_txs_distance: Option<u64>,

    /// Include all receipt static files.
    #[arg(long, conflicts_with_all = ["with_receipts_since", "with_receipts_distance", "minimal", "full", "archive"])]
    with_receipts: bool,

    /// Include receipt static files starting at the specified block.
    #[arg(long, value_name = "BLOCK_NUMBER", conflicts_with_all = ["with_receipts", "with_receipts_distance", "minimal", "full", "archive"])]
    with_receipts_since: Option<u64>,

    /// Include receipt static files covering the last N blocks.
    #[arg(long, value_name = "BLOCKS", value_parser = RangedU64ValueParser::<u64>::new().range(1..), conflicts_with_all = ["with_receipts", "with_receipts_since", "minimal", "full", "archive"])]
    with_receipts_distance: Option<u64>,

    /// Include all account and storage history static files.
    #[arg(long, alias = "with-changesets", conflicts_with_all = ["with_state_history_since", "with_state_history_distance", "minimal", "full", "archive"])]
    with_state_history: bool,

    /// Include account and storage history static files starting at the specified block.
    #[arg(long, value_name = "BLOCK_NUMBER", conflicts_with_all = ["with_state_history", "with_state_history_distance", "minimal", "full", "archive"])]
    with_state_history_since: Option<u64>,

    /// Include account and storage history static files covering the last N blocks.
    #[arg(long, value_name = "BLOCKS", value_parser = RangedU64ValueParser::<u64>::new().range(1..), conflicts_with_all = ["with_state_history", "with_state_history_since", "minimal", "full", "archive"])]
    with_state_history_distance: Option<u64>,

    /// Include transaction sender static files. Requires `--with-txs`.
    #[arg(long, requires = "with_txs", conflicts_with_all = ["minimal", "full", "archive"])]
    with_senders: bool,

    /// Include RocksDB index files.
    #[arg(long, conflicts_with_all = ["minimal", "full", "archive", "without_rocksdb"])]
    with_rocksdb: bool,

    /// Download all available components (archive node, no pruning).
    #[arg(long, alias = "all", conflicts_with_all = ["with_txs", "with_txs_since", "with_txs_distance", "with_receipts", "with_receipts_since", "with_receipts_distance", "with_state_history", "with_state_history_since", "with_state_history_distance", "with_senders", "with_rocksdb", "minimal", "full"])]
    archive: bool,

    /// Download the minimal component set (same default as --non-interactive).
    #[arg(long, conflicts_with_all = ["with_txs", "with_txs_since", "with_txs_distance", "with_receipts", "with_receipts_since", "with_receipts_distance", "with_state_history", "with_state_history_since", "with_state_history_distance", "with_senders", "with_rocksdb", "archive", "full"])]
    minimal: bool,

    /// Download the full node component set (matches default full prune settings).
    #[arg(long, conflicts_with_all = ["with_txs", "with_txs_since", "with_txs_distance", "with_receipts", "with_receipts_since", "with_receipts_distance", "with_state_history", "with_state_history_since", "with_state_history_distance", "with_senders", "with_rocksdb", "archive", "minimal"])]
    full: bool,

    /// Skip optional RocksDB indices even when archive components are selected.
    ///
    /// This affects `--archive`/`--all` and TUI archive preset (`a`).
    #[arg(long, conflicts_with_all = ["url", "with_rocksdb"])]
    without_rocksdb: bool,

    /// Skip interactive component selection. Downloads the minimal set
    /// (state + headers + transactions + changesets) unless explicit --with-* flags narrow it.
    #[arg(long, short = 'y')]
    non_interactive: bool,

    /// Enable resumable two-phase downloads (download to disk first, then extract).
    ///
    /// Archives are downloaded to a `.part` file with HTTP Range resume support
    /// before extraction. This is enabled by default because it tolerates
    /// network interruptions without restarting. Pass `--resumable=false` to
    /// stream archives directly into the extractor instead.
    #[arg(long, default_value_t = true, num_args = 0..=1, default_missing_value = "true")]
    resumable: bool,

    /// Maximum number of simultaneous HTTP downloads.
    ///
    /// Applies across the entire snapshot download. Small files use one slot,
    /// while large files may use multiple slots by splitting into fixed-size pieces.
    #[arg(long, default_value_t = MAX_CONCURRENT_DOWNLOADS)]
    download_concurrency: usize,

    /// List available snapshots and exit.
    ///
    /// Queries the snapshots API and prints all available snapshots for the selected chain,
    /// including block number, size, and manifest URL.
    #[arg(long, alias = "list-snapshots", conflicts_with_all = ["url", "manifest_url", "manifest_path"])]
    list: bool,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> DownloadCommand<C> {
    /// Runs the download command in single-archive or manifest mode.
    pub async fn execute<N>(self) -> Result<()> {
        let chain = self.env.chain.chain();
        let chain_id = chain.id();
        let data_dir = self.env.datadir.clone().resolve_datadir(chain);
        fs::create_dir_all(&data_dir)?;

        let cancel_token = CancellationToken::new();
        let _cancel_guard = cancel_token.drop_guard();

        // --list: print available snapshots and exit
        if self.list {
            let entries = fetch_snapshot_api_entries(chain_id).await?;
            print_snapshot_listing(&entries, chain_id);
            return Ok(());
        }

        // Legacy single-URL mode: download one archive and extract it
        if let Some(ref url) = self.url {
            let request_limiter = DownloadRequestLimiter::new(self.download_concurrency.max(1));
            info!(target: "reth::cli",
                dir = ?data_dir.data_dir(),
                url = %url,
                "Starting snapshot download and extraction"
            );

            stream_and_extract(
                url,
                data_dir.data_dir(),
                None,
                self.resumable,
                Some(request_limiter),
                cancel_token.clone(),
            )
            .await?;
            info!(target: "reth::cli", "Snapshot downloaded and extracted successfully");

            return Ok(());
        }

        let manifest = self.load_manifest(chain_id).await?;
        let ResolvedComponents { mut selections, preset } = self.resolve_components(&manifest)?;

        if matches!(preset, Some(SelectionPreset::Archive)) {
            inject_archive_only_components(&mut selections, &manifest, !self.without_rocksdb);
        }

        let target_dir = data_dir.data_dir();
        let planned_downloads = collect_planned_archives(&manifest, &selections)?;
        let startup_summary = summarize_download_startup(&planned_downloads.archives, target_dir)?;
        info!(target: "reth::cli",
            reusable = startup_summary.reusable,
            needs_download = startup_summary.needs_download,
            "Startup integrity summary (plain output files)"
        );

        info!(target: "reth::cli",
            archives = planned_downloads.total_archives(),
            download_total = %DownloadProgress::format_size(planned_downloads.total_download_size),
            output_total = %DownloadProgress::format_size(planned_downloads.total_output_size),
            "Downloading all archives"
        );

        run_modular_downloads(
            planned_downloads,
            target_dir,
            self.download_concurrency.max(1),
            cancel_token.clone(),
        )
        .await?;

        self.finalize_modular_download(&selections, &manifest, preset, target_dir, &data_dir.db())?;

        Ok(())
    }

    /// Loads the manifest and resolves its effective base URL.
    async fn load_manifest(&self, chain_id: u64) -> Result<SnapshotManifest> {
        let manifest_source = self.resolve_manifest_source(chain_id).await?;

        info!(target: "reth::cli", source = %manifest_source, "Fetching snapshot manifest");
        let mut manifest = fetch_manifest_from_source(&manifest_source).await?;
        manifest.base_url = Some(resolve_manifest_base_url(&manifest, &manifest_source)?);

        info!(target: "reth::cli",
            block = manifest.block,
            chain_id = manifest.chain_id,
            storage_version = %manifest.storage_version,
            components = manifest.components.len(),
            "Loaded snapshot manifest"
        );

        Ok(manifest)
    }

    /// Writes config and checkpoint state after all modular archives complete.
    fn finalize_modular_download(
        &self,
        selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
        manifest: &SnapshotManifest,
        preset: Option<SelectionPreset>,
        target_dir: &Path,
        db_path: &Path,
    ) -> Result<()> {
        let config =
            config_for_selections(selections, manifest, preset, Some(self.env.chain.as_ref()));
        if write_config(&config, target_dir)? {
            let desc = config_gen::describe_prune_config(&config);
            info!(target: "reth::cli", "{}", desc.join(", "));
        }

        let db = init_db(db_path, self.env.db.database_args())?;
        let should_write_prune = config.prune.segments != Default::default();
        let should_reset_indices = should_reset_index_stage_checkpoints(selections);
        if should_write_prune || should_reset_indices {
            let tx = db.tx_mut()?;

            if should_write_prune {
                config_gen::write_prune_checkpoints_tx(&tx, &config, manifest.block)?;
            }

            if should_reset_indices {
                config_gen::reset_index_stage_checkpoints_tx(&tx)?;
            }

            tx.commit()?;
        }

        let start_command = startup_node_command::<C>(self.env.chain.as_ref());
        info!(target: "reth::cli", "Snapshot download complete. Run `{}` to start syncing.", start_command);

        Ok(())
    }

    /// Determines which components to download based on CLI flags or interactive selection.
    fn resolve_components(&self, manifest: &SnapshotManifest) -> Result<ResolvedComponents> {
        let available = |ty: SnapshotComponentType| manifest.component(ty).is_some();

        // --archive/--all: everything available as All
        if self.archive {
            return Ok(ResolvedComponents {
                selections: SnapshotComponentType::ALL
                    .iter()
                    .copied()
                    .filter(|ty| available(*ty))
                    .filter(|ty| {
                        !self.without_rocksdb || *ty != SnapshotComponentType::RocksdbIndices
                    })
                    .map(|ty| (ty, ComponentSelection::All))
                    .collect(),
                preset: Some(SelectionPreset::Archive),
            });
        }

        if self.full {
            return Ok(ResolvedComponents {
                selections: self.full_preset_selections(manifest),
                preset: Some(SelectionPreset::Full),
            });
        }

        if self.minimal {
            return Ok(ResolvedComponents {
                selections: self.minimal_preset_selections(manifest),
                preset: Some(SelectionPreset::Minimal),
            });
        }

        let has_explicit_flags = self.with_txs ||
            self.with_txs_since.is_some() ||
            self.with_txs_distance.is_some() ||
            self.with_receipts ||
            self.with_receipts_since.is_some() ||
            self.with_receipts_distance.is_some() ||
            self.with_state_history ||
            self.with_state_history_since.is_some() ||
            self.with_state_history_distance.is_some() ||
            self.with_senders ||
            self.with_rocksdb;

        if has_explicit_flags {
            let mut selections = BTreeMap::new();
            let tx_selection = explicit_component_selection(
                self.with_txs,
                self.with_txs_since,
                self.with_txs_distance,
                manifest.block,
            );
            let receipt_selection = explicit_component_selection(
                self.with_receipts,
                self.with_receipts_since,
                self.with_receipts_distance,
                manifest.block,
            );
            let state_history_selection = explicit_component_selection(
                self.with_state_history,
                self.with_state_history_since,
                self.with_state_history_distance,
                manifest.block,
            );

            // Required components always All
            if available(SnapshotComponentType::State) {
                selections.insert(SnapshotComponentType::State, ComponentSelection::All);
            }
            if available(SnapshotComponentType::Headers) {
                selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
            }
            if let Some(selection) = tx_selection &&
                available(SnapshotComponentType::Transactions)
            {
                selections.insert(SnapshotComponentType::Transactions, selection);
            }
            if let Some(selection) = receipt_selection &&
                available(SnapshotComponentType::Receipts)
            {
                selections.insert(SnapshotComponentType::Receipts, selection);
            }
            if let Some(selection) = state_history_selection {
                if available(SnapshotComponentType::AccountChangesets) {
                    selections.insert(SnapshotComponentType::AccountChangesets, selection);
                }
                if available(SnapshotComponentType::StorageChangesets) {
                    selections.insert(SnapshotComponentType::StorageChangesets, selection);
                }
            }
            if self.with_senders && available(SnapshotComponentType::TransactionSenders) {
                selections
                    .insert(SnapshotComponentType::TransactionSenders, ComponentSelection::All);
            }
            if self.with_rocksdb && available(SnapshotComponentType::RocksdbIndices) {
                selections.insert(SnapshotComponentType::RocksdbIndices, ComponentSelection::All);
            }
            return Ok(ResolvedComponents { selections, preset: None });
        }

        if self.non_interactive {
            return Ok(ResolvedComponents {
                selections: self.minimal_preset_selections(manifest),
                preset: Some(SelectionPreset::Minimal),
            });
        }

        // Interactive TUI
        let full_preset = self.full_preset_selections(manifest);
        let SelectorOutput { selections, preset } = run_selector(manifest.clone(), &full_preset)?;
        let selected =
            selections.into_iter().filter(|(_, sel)| *sel != ComponentSelection::None).collect();

        Ok(ResolvedComponents { selections: selected, preset })
    }

    /// Builds the default minimal component selection for the manifest.
    fn minimal_preset_selections(
        &self,
        manifest: &SnapshotManifest,
    ) -> BTreeMap<SnapshotComponentType, ComponentSelection> {
        SnapshotComponentType::ALL
            .iter()
            .copied()
            .filter(|ty| manifest.component(*ty).is_some())
            .map(|ty| (ty, ty.minimal_selection()))
            .collect()
    }

    /// Builds the default full-node component selection for the manifest.
    fn full_preset_selections(
        &self,
        manifest: &SnapshotManifest,
    ) -> BTreeMap<SnapshotComponentType, ComponentSelection> {
        let mut selections = BTreeMap::new();

        for ty in [
            SnapshotComponentType::State,
            SnapshotComponentType::Headers,
            SnapshotComponentType::Transactions,
            SnapshotComponentType::Receipts,
            SnapshotComponentType::AccountChangesets,
            SnapshotComponentType::StorageChangesets,
            SnapshotComponentType::TransactionSenders,
            SnapshotComponentType::RocksdbIndices,
        ] {
            if manifest.component(ty).is_none() {
                continue;
            }

            let selection = self.full_selection_for_component(ty, manifest.block);
            if selection != ComponentSelection::None {
                selections.insert(ty, selection);
            }
        }

        selections
    }

    /// Returns the full preset selection for one component type.
    fn full_selection_for_component(
        &self,
        ty: SnapshotComponentType,
        snapshot_block: u64,
    ) -> ComponentSelection {
        let defaults = DefaultPruningValues::get_global();
        match ty {
            SnapshotComponentType::State | SnapshotComponentType::Headers => {
                ComponentSelection::All
            }
            SnapshotComponentType::Transactions => {
                if defaults.full_bodies_history_use_pre_merge {
                    match self
                        .env
                        .chain
                        .ethereum_fork_activation(EthereumHardfork::Paris)
                        .block_number()
                    {
                        Some(paris) if snapshot_block >= paris => ComponentSelection::Since(paris),
                        Some(_) => ComponentSelection::None,
                        None => ComponentSelection::All,
                    }
                } else {
                    selection_from_prune_mode(
                        defaults.full_prune_modes.bodies_history,
                        snapshot_block,
                    )
                }
            }
            SnapshotComponentType::Receipts => {
                selection_from_prune_mode(defaults.full_prune_modes.receipts, snapshot_block)
            }
            SnapshotComponentType::AccountChangesets => {
                selection_from_prune_mode(defaults.full_prune_modes.account_history, snapshot_block)
            }
            SnapshotComponentType::StorageChangesets => {
                selection_from_prune_mode(defaults.full_prune_modes.storage_history, snapshot_block)
            }
            SnapshotComponentType::TransactionSenders => {
                selection_from_prune_mode(defaults.full_prune_modes.sender_recovery, snapshot_block)
            }
            // Keep hidden by default in full mode; if users want indices they can use archive.
            SnapshotComponentType::RocksdbIndices => ComponentSelection::None,
        }
    }

    /// Resolves the manifest source from CLI input or snapshot discovery.
    async fn resolve_manifest_source(&self, chain_id: u64) -> Result<String> {
        if let Some(path) = &self.manifest_path {
            return Ok(path.display().to_string());
        }

        match &self.manifest_url {
            Some(url) => Ok(url.clone()),
            None => {
                let defaults = DownloadDefaults::get_global();
                if defaults.mainnet_only_discovery() && chain_id != MAINNET.chain.id() {
                    eyre::bail!(
                        "Snapshots are only auto-discovered for Ethereum mainnet.\n\n\
                         Chain {chain_id} requires an explicit source:\n\
                         \t--manifest-url <URL>\n\
                         \t--manifest-path <PATH>\n\
                         \t-u <SNAPSHOT-URL>\n\n\
                         Use --list to inspect snapshots exposed by {}.",
                        defaults.snapshot_api_url.trim_end_matches("/api/snapshots"),
                    );
                }

                discover_manifest_url(chain_id).await
            }
        }
    }
}

/// Resolves explicit `--with-*` / `--with-*-since` / `--with-*-distance` flags
/// into a component selection.
fn explicit_component_selection(
    all: bool,
    since: Option<u64>,
    distance: Option<u64>,
    snapshot_block: u64,
) -> Option<ComponentSelection> {
    if all {
        Some(ComponentSelection::All)
    } else if let Some(block) = since {
        (block <= snapshot_block).then_some(ComponentSelection::Since(block))
    } else {
        distance.map(ComponentSelection::Distance)
    }
}

/// Converts a prune mode into the matching component selection.
fn selection_from_prune_mode(mode: Option<PruneMode>, snapshot_block: u64) -> ComponentSelection {
    match mode {
        None => ComponentSelection::All,
        Some(PruneMode::Full) => ComponentSelection::None,
        Some(PruneMode::Distance(d)) => ComponentSelection::Distance(d),
        Some(PruneMode::Before(block)) => {
            if snapshot_block >= block {
                ComponentSelection::Since(block)
            } else {
                ComponentSelection::None
            }
        }
    }
}

/// If all data components (txs, receipts, changesets) are `All`, automatically
/// include hidden archive-only components when available in the manifest.
fn inject_archive_only_components(
    selections: &mut BTreeMap<SnapshotComponentType, ComponentSelection>,
    manifest: &SnapshotManifest,
    include_rocksdb: bool,
) {
    let is_all =
        |ty: SnapshotComponentType| selections.get(&ty).copied() == Some(ComponentSelection::All);

    let is_archive = is_all(SnapshotComponentType::Transactions) &&
        is_all(SnapshotComponentType::Receipts) &&
        is_all(SnapshotComponentType::AccountChangesets) &&
        is_all(SnapshotComponentType::StorageChangesets);

    if !is_archive {
        return;
    }

    for component in
        [SnapshotComponentType::TransactionSenders, SnapshotComponentType::RocksdbIndices]
    {
        if component == SnapshotComponentType::RocksdbIndices && !include_rocksdb {
            continue;
        }

        if manifest.component(component).is_some() {
            selections.insert(component, ComponentSelection::All);
        }
    }
}

/// Returns `true` when RocksDB-backed index stages should be reset after download.
fn should_reset_index_stage_checkpoints(
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> bool {
    !matches!(selections.get(&SnapshotComponentType::RocksdbIndices), Some(ComponentSelection::All))
}

fn startup_node_command<C>(chain_spec: &C::ChainSpec) -> String
where
    C: ChainSpecParser,
    C::ChainSpec: EthChainSpec,
{
    startup_node_command_for_binary::<C>(&current_binary_name(), chain_spec)
}

fn startup_node_command_for_binary<C>(binary_name: &str, chain_spec: &C::ChainSpec) -> String
where
    C: ChainSpecParser,
    C::ChainSpec: EthChainSpec,
{
    let mut command = format!("{binary_name} node");

    if let Some(chain_arg) = startup_chain_arg::<C>(chain_spec) {
        command.push_str(" --chain ");
        command.push_str(&chain_arg);
    }

    command
}

fn current_binary_name() -> String {
    std::env::args_os()
        .next()
        .map(PathBuf::from)
        .and_then(|path| path.file_stem().map(|name| name.to_owned()))
        .and_then(|name| name.into_string().ok())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "reth".to_string())
}

fn startup_chain_arg<C>(chain_spec: &C::ChainSpec) -> Option<String>
where
    C: ChainSpecParser,
    C::ChainSpec: EthChainSpec,
{
    let current_chain = chain_spec.chain();
    let current_genesis_hash = chain_spec.genesis_hash();
    let default_chain = C::default_value().and_then(|chain_name| C::parse(chain_name).ok());

    if default_chain.as_ref().is_some_and(|default_chain| {
        default_chain.chain() == current_chain &&
            default_chain.genesis_hash() == current_genesis_hash
    }) {
        return None;
    }

    C::SUPPORTED_CHAINS
        .iter()
        .find_map(|chain_name| {
            let parsed_chain = C::parse(chain_name).ok()?;
            (parsed_chain.chain() == current_chain &&
                parsed_chain.genesis_hash() == current_genesis_hash)
                .then(|| (*chain_name).to_string())
        })
        .or_else(|| Some("<chain-or-chainspec>".to_string()))
}

impl<C: ChainSpecParser> DownloadCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

const MAX_DOWNLOAD_RETRIES: u32 = 10;
const RETRY_BACKOFF_SECS: u64 = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};
    use extract::CompressionFormat;
    use manifest::{ComponentManifest, SingleArchive};
    use reth_chainspec::{HOLESKY, MAINNET};
    use reth_ethereum_cli::chainspec::EthereumChainSpecParser;

    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    fn manifest_with_archive_only_components() -> SnapshotManifest {
        let mut components = BTreeMap::new();
        components.insert(
            SnapshotComponentType::TransactionSenders.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "transaction_senders.tar.zst".to_string(),
                size: 1,
                decompressed_size: 0,
                blake3: None,
                output_files: vec![],
            }),
        );
        components.insert(
            SnapshotComponentType::RocksdbIndices.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "rocksdb_indices.tar.zst".to_string(),
                size: 1,
                decompressed_size: 0,
                blake3: None,
                output_files: vec![],
            }),
        );
        SnapshotManifest {
            block: 0,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: Some("https://example.com".to_string()),
            reth_version: None,
            components,
        }
    }

    #[test]
    fn test_download_defaults_builder() {
        let defaults = DownloadDefaults::default()
            .with_snapshot("https://example.com/snapshots (example)")
            .with_base_url("https://example.com");

        assert_eq!(defaults.default_base_url, "https://example.com");
        assert_eq!(defaults.available_snapshots.len(), 3); // 2 defaults + 1 added
    }

    #[test]
    fn test_download_defaults_replace_snapshots() {
        let defaults = DownloadDefaults::default().with_snapshots(vec![
            Cow::Borrowed("https://custom1.com"),
            Cow::Borrowed("https://custom2.com"),
        ]);

        assert_eq!(defaults.available_snapshots.len(), 2);
        assert_eq!(defaults.available_snapshots[0], "https://custom1.com");
    }

    #[test]
    fn test_long_help_generation() {
        let defaults = DownloadDefaults::default();
        let help = defaults.long_help();

        assert!(help.contains("Available snapshot sources:"));
        assert!(help.contains("Ethereum mainnet"));
        assert!(help.contains("snapshots.reth.rs"));
        assert!(help.contains("publicnode.com"));
        assert!(help.contains("file://"));
    }

    #[test]
    fn test_custom_snapshot_api_keeps_selected_chain_help() {
        let help = DownloadDefaults::default()
            .with_snapshot_api_url("https://snapshots.tempoxyz.dev/api/snapshots")
            .long_help();

        assert!(help.contains("selected chain"));
        assert!(!help.contains("Ethereum mainnet"));
    }

    #[test]
    fn test_long_help_override() {
        let custom_help = "This is custom help text for downloading snapshots.";
        let defaults = DownloadDefaults::default().with_long_help(custom_help);

        let help = defaults.long_help();
        assert_eq!(help, custom_help);
        assert!(!help.contains("Available snapshot sources:"));
    }

    #[test]
    fn test_builder_chaining() {
        let defaults = DownloadDefaults::default()
            .with_base_url("https://custom.example.com")
            .with_snapshot("https://snapshot1.com")
            .with_snapshot("https://snapshot2.com")
            .with_long_help("Custom help for snapshots");

        assert_eq!(defaults.default_base_url, "https://custom.example.com");
        assert_eq!(defaults.available_snapshots.len(), 4); // 2 defaults + 2 added
        assert_eq!(defaults.long_help, Some("Custom help for snapshots".to_string()));
    }

    #[test]
    fn test_download_resumable_defaults_to_true() {
        let args =
            CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from(["reth"]).args;

        assert!(args.resumable);
    }

    #[test]
    fn test_download_resumable_implicit_true() {
        let args = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from([
            "reth",
            "--resumable",
        ])
        .args;

        assert!(args.resumable);
    }

    #[test]
    fn test_download_resumable_explicit_false() {
        let args = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from([
            "reth",
            "--resumable=false",
        ])
        .args;

        assert!(!args.resumable);
    }

    #[test]
    fn resolve_manifest_source_requires_explicit_source_for_non_mainnet_defaults() {
        let args = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from([
            "reth", "--chain", "holesky",
        ])
        .args;

        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(args.resolve_manifest_source(HOLESKY.chain.id()))
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("only auto-discovered for Ethereum mainnet"));
        assert!(message.contains("--manifest-url <URL>"));
        assert!(message.contains("-u <SNAPSHOT-URL>"));
    }

    #[test]
    fn resolve_manifest_source_allows_manifest_path_for_non_mainnet_defaults() {
        let args = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from([
            "reth",
            "--chain",
            "holesky",
            "--manifest-path",
            "./manifest.json",
        ])
        .args;

        let source = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(args.resolve_manifest_source(HOLESKY.chain.id()))
            .unwrap();

        assert_eq!(source, "./manifest.json");
    }

    #[test]
    fn test_compression_format_detection() {
        assert!(matches!(
            CompressionFormat::from_url("https://example.com/snapshot.tar.lz4"),
            Ok(CompressionFormat::Lz4)
        ));
        assert!(matches!(
            CompressionFormat::from_url("https://example.com/snapshot.tar.zst"),
            Ok(CompressionFormat::Zstd)
        ));
        assert!(matches!(
            CompressionFormat::from_url("file:///path/to/snapshot.tar.lz4"),
            Ok(CompressionFormat::Lz4)
        ));
        assert!(matches!(
            CompressionFormat::from_url("file:///path/to/snapshot.tar.zst"),
            Ok(CompressionFormat::Zstd)
        ));
        assert!(CompressionFormat::from_url("https://example.com/snapshot.tar.gz").is_err());
    }

    #[test]
    fn inject_archive_only_components_for_archive_selection() {
        let manifest = manifest_with_archive_only_components();
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::Transactions, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Receipts, ComponentSelection::All);
        selections.insert(SnapshotComponentType::AccountChangesets, ComponentSelection::All);
        selections.insert(SnapshotComponentType::StorageChangesets, ComponentSelection::All);

        inject_archive_only_components(&mut selections, &manifest, true);

        assert_eq!(
            selections.get(&SnapshotComponentType::TransactionSenders),
            Some(&ComponentSelection::All)
        );
        assert_eq!(
            selections.get(&SnapshotComponentType::RocksdbIndices),
            Some(&ComponentSelection::All)
        );
    }

    #[test]
    fn inject_archive_only_components_without_rocksdb() {
        let manifest = manifest_with_archive_only_components();
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::Transactions, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Receipts, ComponentSelection::All);
        selections.insert(SnapshotComponentType::AccountChangesets, ComponentSelection::All);
        selections.insert(SnapshotComponentType::StorageChangesets, ComponentSelection::All);

        inject_archive_only_components(&mut selections, &manifest, false);

        assert_eq!(
            selections.get(&SnapshotComponentType::TransactionSenders),
            Some(&ComponentSelection::All)
        );
        assert_eq!(selections.get(&SnapshotComponentType::RocksdbIndices), None);
    }

    #[test]
    fn should_reset_index_stage_checkpoints_without_rocksdb_indices() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::Transactions, ComponentSelection::All);
        assert!(should_reset_index_stage_checkpoints(&selections));

        selections.insert(SnapshotComponentType::RocksdbIndices, ComponentSelection::All);
        assert!(!should_reset_index_stage_checkpoints(&selections));
    }

    #[test]
    fn startup_node_command_omits_default_chain_arg() {
        let command =
            startup_node_command_for_binary::<EthereumChainSpecParser>("reth", MAINNET.as_ref());

        assert_eq!(command, "reth node");
    }

    #[test]
    fn startup_node_command_includes_non_default_chain_arg() {
        let command =
            startup_node_command_for_binary::<EthereumChainSpecParser>("reth", HOLESKY.as_ref());

        assert_eq!(command, "reth node --chain holesky");
    }

    #[test]
    fn startup_node_command_uses_running_binary_name() {
        let command =
            startup_node_command_for_binary::<EthereumChainSpecParser>("tempo", HOLESKY.as_ref());

        assert_eq!(command, "tempo node --chain holesky");
    }
}
