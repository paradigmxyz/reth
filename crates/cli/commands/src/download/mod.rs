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
//! The manifest chain ID is checked when the selected chain determines the source, default data
//! directory, or pre-Merge pruning settings. Explicit sources and data directories do not require
//! a matching chain when component selection is independent of its hardforks.
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
//! directly.
//!
//! Before any archive is scheduled, `PlannedDownloads::partition_reusable` hashes the existing
//! output files once and skips archives whose outputs already verify. Each remaining archive is
//! processed as follows:
//!
//! - fetch and extract the archive,
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
mod prune;
mod session;
mod source;
mod tui;
mod verify;

pub use planning::{DownloadPlan, DownloadPlanArchive};

use crate::common::EnvironmentArgs;
use archive::run_modular_downloads;
use clap::{builder::RangedU64ValueParser, Parser};
use config_gen::{config_for_selections, write_config};
use extract::stream_and_extract;
use eyre::Result;
use manifest::{ComponentSelection, SnapshotComponentType, SnapshotManifest};
use planning::{collect_planned_archives, PlannedDownloads};
use progress::{DownloadProgress, DownloadRequestLimiter};
use prune::prune_unlisted_outputs;
use reth_chainspec::{EthChainSpec, EthereumHardfork, EthereumHardforks, MAINNET};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_util::cancellation::CancellationToken;
use reth_config::config::Config;
use reth_db::{init_db, Database};
use reth_db_api::transaction::DbTx;
use reth_fs_util as fs;
use reth_node_core::args::DefaultPruningValues;
use reth_prune_types::{PruneMode, PruneModes, MINIMUM_UNWIND_SAFE_DISTANCE};
use source::{
    discover_manifest_url, fetch_manifest_from_source, fetch_snapshot_api_entries,
    print_snapshot_listing, resolve_manifest_base_url,
};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tracing::info;
use tui::{run_selector, SelectorOutput};

const RETH_SNAPSHOTS_BASE_URL: &str = "https://snapshots-r2.reth.rs";
const RETH_SNAPSHOTS_API_URL: &str = "https://snapshots.reth.rs/api/snapshots";
const RETH_SNAPSHOTS_SOURCE: &str = "https://snapshots.reth.rs (default)";
const SNAPSHOT_API_PATH: &str = "/api/snapshots";
const MANAGED_DATADIR_PATHS: &[&str] = &["db", "rocksdb", "static_files", "reth.toml"];

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
    /// Whether the full preset also downloads the snapshot's RocksDB indices instead of
    /// rebuilding them.
    pub full_preset_includes_rocksdb_indices: bool,
}

impl DownloadDefaults {
    /// Initialize the global download defaults with this configuration
    #[expect(clippy::result_large_err, reason = "returns the rejected defaults to the caller")]
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
                Cow::Borrowed(RETH_SNAPSHOTS_SOURCE),
                Cow::Borrowed("https://publicnode.com/snapshots (full nodes & testnets)"),
            ],
            default_base_url: Cow::Borrowed(RETH_SNAPSHOTS_BASE_URL),
            default_chain_aware_base_url: None,
            snapshot_api_url: Cow::Borrowed(RETH_SNAPSHOTS_API_URL),
            long_help: None,
            full_preset_includes_rocksdb_indices: false,
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
            self.snapshot_source_url(),
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
        self.snapshot_api_url.trim_end_matches('/') == RETH_SNAPSHOTS_API_URL
    }

    fn snapshot_source_url(&self) -> &str {
        snapshot_source_url_from_api(&self.snapshot_api_url)
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
    ///
    /// Generated help uses the API root as the default snapshot source unless a custom
    /// chain-aware base URL or source list was already provided.
    pub fn with_snapshot_api_url(mut self, url: impl Into<Cow<'static, str>>) -> Self {
        self.snapshot_api_url = url.into();

        let source_url = self.snapshot_source_url().to_string();
        if self.default_chain_aware_base_url.is_none() {
            self.default_chain_aware_base_url = Some(Cow::Owned(source_url.clone()));
        }
        for source in &mut self.available_snapshots {
            if source.as_ref() == RETH_SNAPSHOTS_SOURCE {
                *source = Cow::Owned(format!("{source_url} (default)"));
            }
        }

        self
    }

    /// Set one default snapshot source URL for discovery and generated CLI references.
    ///
    /// The provided URL is the public snapshot root, such as `https://snapshots.example.com`.
    /// The discovery API is derived as `{url}/api/snapshots`.
    pub fn with_snapshot_source_url(mut self, url: impl Into<Cow<'static, str>>) -> Self {
        let source_url = normalize_snapshot_source_url(url.into());
        self.available_snapshots = vec![Cow::Owned(format!("{} (default)", source_url.as_ref()))];
        self.default_base_url = source_url.clone();
        self.default_chain_aware_base_url = Some(source_url.clone());
        self.snapshot_api_url = Cow::Owned(format!("{}{SNAPSHOT_API_PATH}", source_url.as_ref()));
        self
    }

    /// Builder: Make the full preset also download the snapshot's RocksDB indices.
    pub const fn with_full_preset_rocksdb_indices(mut self) -> Self {
        self.full_preset_includes_rocksdb_indices = true;
        self
    }

    /// Builder: Set custom long help text, overriding the generated help
    pub fn with_long_help(mut self, help: impl Into<String>) -> Self {
        self.long_help = Some(help.into());
        self
    }
}

fn snapshot_source_url_from_api(api_url: &str) -> &str {
    api_url.trim_end_matches('/').trim_end_matches(SNAPSHOT_API_PATH)
}

fn normalize_snapshot_source_url(url: Cow<'static, str>) -> Cow<'static, str> {
    match url {
        Cow::Borrowed(url) => Cow::Borrowed(snapshot_source_url_from_api(url)),
        Cow::Owned(url) => Cow::Owned(snapshot_source_url_from_api(&url).to_string()),
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
    /// This affects `--archive`/`--all`, TUI archive preset (`a`), and the full preset when it
    /// includes RocksDB indices.
    #[arg(long, conflicts_with_all = ["url", "with_rocksdb"])]
    without_rocksdb: bool,

    /// Skip interactive component selection. Downloads the minimal set
    /// (state + headers + transactions + changesets) unless explicit --with-* flags narrow it.
    #[arg(long, short = 'y')]
    non_interactive: bool,

    /// Overwrite existing snapshot data by removing db, rocksdb, static_files, and reth.toml.
    #[arg(long, conflicts_with = "list")]
    force: bool,

    /// Remove files from db, rocksdb, static_files, and reth.toml that the selected snapshot
    /// plan does not list, then reuse the listed files that still verify.
    ///
    /// Unlike `--force`, this keeps reusable snapshot files, so only missing or changed archives
    /// are downloaded. Unlike a plain re-run, the data dir ends up with exactly the snapshot's
    /// files, which is required when the node has written data past the snapshot block.
    ///
    /// Components that are not selected are removed entirely, so `--prune-unlisted --minimal`
    /// over an archive node deletes its history. With `--non-interactive`, components must be
    /// selected explicitly with a preset or `--with-*` flags. reth.toml is always regenerated,
    /// which discards local edits. Symlinks below these paths are removed, not followed. Pruning
    /// refuses to run while a node holds the database lock, and happens before downloading, so a
    /// failed download leaves the data dir pruned, as with `--force`.
    #[arg(long, conflicts_with_all = ["force", "list", "url"])]
    prune_unlisted: bool,

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

    /// Override the delay between retry attempts (for example, 500ms or 5s).
    ///
    /// Applies to requests, extraction, output verification, and segmented downloads.
    /// By default, retries wait five seconds; segmented requests use adaptive backoff.
    /// This does not change the number of attempts.
    #[arg(long, value_name = "DURATION", value_parser = reth_cli_util::parse_duration_from_secs_or_ms)]
    retry_backoff: Option<Duration>,

    /// List available snapshots and exit.
    ///
    /// Queries the snapshots API and prints all available snapshots for the selected chain,
    /// including block number, size, and manifest URL.
    #[arg(long, alias = "list-snapshots", conflicts_with_all = ["url", "manifest_url", "manifest_path"])]
    list: bool,

    /// Print the selected modular archive plan as JSON and exit without downloading.
    #[arg(long, conflicts_with_all = ["url", "list"])]
    print_plan_json: bool,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> DownloadCommand<C> {
    /// Runs the download command in single-archive or manifest mode.
    pub async fn execute<N>(self) -> Result<Option<PreparedSnapshotDownload>> {
        let chain = self.env.chain.chain();

        // --list: print available snapshots and exit
        if self.list {
            let entries = fetch_snapshot_api_entries(chain.id()).await?;
            print_snapshot_listing(&entries, chain.id());
            return Ok(None);
        }

        let data_dir = self.env.datadir.clone().resolve_datadir(chain);
        let static_files_dir = data_dir.static_files();
        let static_files_dir = (static_files_dir != data_dir.data_dir().join("static_files"))
            .then_some(static_files_dir);

        // Legacy single-URL mode: download one archive and extract it
        if let Some(ref url) = self.url {
            let cancel_token = CancellationToken::new();
            let _cancel_guard = cancel_token.drop_guard();
            let data_dir = self.env.datadir.clone().resolve_datadir(chain);
            let target_dir = data_dir.data_dir();
            if self.force {
                clear_existing_datadir(target_dir, static_files_dir.as_deref())?;
            }
            fs::create_dir_all(target_dir)?;

            let request_limiter = DownloadRequestLimiter::new(self.download_concurrency.max(1));
            info!(target: "reth::cli",
                dir = ?data_dir.data_dir(),
                url = %url,
                "Starting snapshot download and extraction"
            );

            stream_and_extract(
                url,
                data_dir.data_dir(),
                static_files_dir.as_deref(),
                self.resumable,
                Some(request_limiter),
                cancel_token.clone(),
                self.retry_backoff,
            )
            .await?;
            info!(target: "reth::cli", "Snapshot downloaded and extracted successfully");

            return Ok(None);
        }

        let ResolvedDownload { manifest, selections, preset, planned } =
            self.resolve_download(chain.id()).await?;
        let data_dir = self.env.datadir.clone().resolve_datadir(chain).data_dir().to_path_buf();
        let prepared = PreparedSnapshotDownload { manifest, data_dir };
        if self.print_plan_json {
            DownloadPlan::from_planned(&prepared.manifest, &planned)
                .write_json(std::io::stdout().lock())?;
            return Ok(Some(prepared))
        }

        let target_dir = prepared.data_dir.as_path();
        let cancel_token = CancellationToken::new();
        let _cancel_guard = cancel_token.drop_guard();
        if self.force {
            clear_existing_datadir(target_dir, static_files_dir.as_deref())?;
        } else if self.prune_unlisted {
            prune_unlisted_outputs(&planned.archives, target_dir, static_files_dir.as_deref())?;
        }
        fs::create_dir_all(target_dir)?;
        let downloads = {
            let (target_dir, static_files_dir) =
                (target_dir.to_path_buf(), static_files_dir.clone());
            reth_tasks::spawn_blocking(move || {
                planned.partition_reusable(&target_dir, static_files_dir.as_deref())
            })
            .await??
        };
        info!(target: "reth::cli",
            reusable = downloads.reused.len(),
            needs_download = downloads.pending.len(),
            "Startup integrity summary (plain output files)"
        );

        info!(target: "reth::cli",
            archives = downloads.total_archives(),
            download_total = %DownloadProgress::format_size(downloads.total_download_size),
            output_total = %DownloadProgress::format_size(downloads.total_output_size),
            "Downloading all archives"
        );

        run_modular_downloads(
            downloads,
            target_dir,
            static_files_dir.as_deref(),
            self.download_concurrency.max(1),
            cancel_token,
            self.retry_backoff,
        )
        .await?;

        let config = self.download_config(&selections, &prepared.manifest, preset);
        self.finalize_modular_download(&selections, &prepared.manifest, &config, target_dir)?;

        Ok(Some(prepared))
    }

    /// Resolves the exact modular archive plan and manifest context without downloading or
    /// modifying the data dir.
    pub async fn plan(&self) -> Result<(DownloadPlan, PreparedSnapshotDownload)> {
        let chain = self.env.chain.chain();
        let resolved = self.resolve_download(chain.id()).await?;
        let plan = DownloadPlan::from_planned(&resolved.manifest, &resolved.planned);
        let prepared = PreparedSnapshotDownload {
            manifest: resolved.manifest,
            data_dir: self.env.datadir.clone().resolve_datadir(chain).data_dir().to_path_buf(),
        };
        Ok((plan, prepared))
    }

    async fn resolve_download(&self, chain_id: u64) -> Result<ResolvedDownload> {
        let manifest = self.load_manifest(chain_id).await?;
        let ResolvedComponents { mut selections, preset } = self.resolve_components(&manifest)?;

        match preset {
            Some(SelectionPreset::Archive) => {
                inject_archive_only_components(&mut selections, &manifest, !self.without_rocksdb)
            }
            Some(SelectionPreset::Full)
                if DownloadDefaults::get_global().full_preset_includes_rocksdb_indices &&
                    !self.without_rocksdb =>
            {
                inject_rocksdb_indices(&mut selections, &manifest)
            }
            _ => {}
        }

        let planned = collect_planned_archives(&manifest, &selections)?;
        Ok(ResolvedDownload { manifest, selections, preset, planned })
    }

    /// Loads the manifest and resolves its effective base URL.
    async fn load_manifest(&self, chain_id: u64) -> Result<SnapshotManifest> {
        let manifest_source = self.resolve_manifest_source(chain_id).await?;

        info!(target: "reth::cli", source = %manifest_source, "Fetching snapshot manifest");
        let mut manifest = fetch_manifest_from_source(&manifest_source).await?;
        if self.env.datadir.datadir.as_ref().is_none() ||
            (self.manifest_url.is_none() && self.manifest_path.is_none())
        {
            self.validate_manifest_chain(&manifest)?;
        }
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

    fn download_config(
        &self,
        selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
        manifest: &SnapshotManifest,
        preset: Option<SelectionPreset>,
    ) -> Config {
        let mut config =
            config_for_selections(selections, manifest, preset, Some(self.env.chain.as_ref()));
        // Use the requested policy even when its range starts beyond the snapshot.
        if let Some(selection) = self.explicit_history_selection() {
            let mode =
                config_gen::selection_to_prune_mode(selection, Some(MINIMUM_UNWIND_SAFE_DISTANCE));
            config.prune.segments.account_history = mode;
            config.prune.segments.storage_history = mode;
        }
        config
    }

    /// Writes config and checkpoint state after all modular archives complete.
    fn finalize_modular_download(
        &self,
        selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
        manifest: &SnapshotManifest,
        config: &Config,
        target_dir: &Path,
    ) -> Result<()> {
        let db = init_db(target_dir.join("db"), self.env.db.database_args())?;
        let tx = db.tx_mut()?;
        let is_v2 = config_gen::snapshot_uses_v2(&tx, selections)?;
        config_gen::write_prune_checkpoints_tx(&tx, config, selections, manifest, is_v2)?;
        if is_v2 && rocksdb_indices_missing(selections) {
            config_gen::reset_index_stage_checkpoints_tx(&tx)?;
        }
        if write_config(config, target_dir)? {
            let desc = config_gen::describe_prune_config(config);
            info!(target: "reth::cli", "{}", desc.join(", "));
        }
        tx.commit()?;

        if self.env.chain.chain().id() == manifest.chain_id {
            let start_command = startup_node_command::<C>(self.env.chain.as_ref());
            info!(target: "reth::cli", "Snapshot download complete. Run `{}` to start syncing.", start_command);
        } else {
            info!(target: "reth::cli", "Snapshot download complete.");
        }

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
                selections: self.full_preset_selections(manifest)?,
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
            );
            let receipt_selection = explicit_component_selection(
                self.with_receipts,
                self.with_receipts_since,
                self.with_receipts_distance,
            );
            let history_selection = self.explicit_history_selection();
            // Required components always All
            if available(SnapshotComponentType::State) {
                selections.insert(SnapshotComponentType::State, ComponentSelection::All);
            }
            if available(SnapshotComponentType::Headers) {
                selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
            }
            for (ty, selection) in [
                (SnapshotComponentType::Transactions, tx_selection),
                (SnapshotComponentType::Receipts, receipt_selection),
                (SnapshotComponentType::AccountChangesets, history_selection),
                (SnapshotComponentType::StorageChangesets, history_selection),
            ] {
                if let Some(selection) = selection &&
                    available(ty) &&
                    !matches!(selection, ComponentSelection::Since(block) if block > manifest.block)
                {
                    selections.insert(ty, selection);
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
            // The implicit minimal default would silently prune history from a full or archive
            // node.
            eyre::ensure!(
                !self.prune_unlisted,
                "--prune-unlisted with --non-interactive requires an explicit component selection: \
                 --minimal, --full, --archive, or --with-* flags"
            );
            return Ok(ResolvedComponents {
                selections: self.minimal_preset_selections(manifest),
                preset: Some(SelectionPreset::Minimal),
            });
        }

        // Interactive TUI
        let minimal_preset = self.minimal_preset_selections(manifest);
        let full_preset = self.full_preset_selections(manifest)?;
        let SelectorOutput { selections, preset } =
            run_selector(manifest.clone(), &minimal_preset, &full_preset)?;
        let selected =
            selections.into_iter().filter(|(_, sel)| *sel != ComponentSelection::None).collect();

        Ok(ResolvedComponents { selections: selected, preset })
    }

    fn explicit_history_selection(&self) -> Option<ComponentSelection> {
        explicit_component_selection(
            self.with_state_history,
            self.with_state_history_since,
            self.with_state_history_distance,
        )
    }

    /// Builds the default minimal component selection for the manifest.
    fn minimal_preset_selections(
        &self,
        manifest: &SnapshotManifest,
    ) -> BTreeMap<SnapshotComponentType, ComponentSelection> {
        self.pruning_preset_selections(manifest, SelectionPreset::Minimal)
    }

    /// Builds the default full-node component selection for the manifest.
    fn full_preset_selections(
        &self,
        manifest: &SnapshotManifest,
    ) -> Result<BTreeMap<SnapshotComponentType, ComponentSelection>> {
        if DefaultPruningValues::get_global().full_bodies_history_use_pre_merge {
            self.validate_manifest_chain(manifest)?;
        }
        Ok(self.pruning_preset_selections(manifest, SelectionPreset::Full))
    }

    /// Builds component selections from the configured pruning preset.
    fn pruning_preset_selections(
        &self,
        manifest: &SnapshotManifest,
        preset: SelectionPreset,
    ) -> BTreeMap<SnapshotComponentType, ComponentSelection> {
        let defaults = DefaultPruningValues::get_global();
        let (prune_modes, bodies_history_use_pre_merge) = match preset {
            SelectionPreset::Minimal => (&defaults.minimal_prune_modes, false),
            SelectionPreset::Full => {
                (&defaults.full_prune_modes, defaults.full_bodies_history_use_pre_merge)
            }
            SelectionPreset::Archive => unreachable!("archive selects every component"),
        };
        let mut selections = BTreeMap::new();

        for &ty in &SnapshotComponentType::ALL {
            if manifest.component(ty).is_none() {
                continue;
            }

            let selection = self.pruning_selection_for_component(
                ty,
                manifest.block,
                prune_modes,
                bodies_history_use_pre_merge,
            );
            if selection != ComponentSelection::None {
                selections.insert(ty, selection);
            }
        }

        selections
    }

    /// Returns the component selection for one configured pruning preset.
    fn pruning_selection_for_component(
        &self,
        ty: SnapshotComponentType,
        snapshot_block: u64,
        prune_modes: &PruneModes,
        bodies_history_use_pre_merge: bool,
    ) -> ComponentSelection {
        if ty == SnapshotComponentType::Transactions && bodies_history_use_pre_merge {
            return match self
                .env
                .chain
                .ethereum_fork_activation(EthereumHardfork::Paris)
                .block_number()
            {
                Some(paris) if snapshot_block >= paris => ComponentSelection::Since(paris),
                Some(_) => ComponentSelection::None,
                None => ComponentSelection::All,
            }
        }

        let mode = match ty {
            SnapshotComponentType::State | SnapshotComponentType::Headers => {
                return ComponentSelection::All
            }
            SnapshotComponentType::Transactions => prune_modes.bodies_history,
            SnapshotComponentType::TransactionSenders => prune_modes.sender_recovery,
            SnapshotComponentType::Receipts => prune_modes.receipts,
            SnapshotComponentType::AccountChangesets => prune_modes.account_history,
            SnapshotComponentType::StorageChangesets => prune_modes.storage_history,
            SnapshotComponentType::RocksdbIndices => return ComponentSelection::None,
        };

        selection_from_prune_mode(mode, snapshot_block)
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
                        defaults.snapshot_source_url(),
                    );
                }

                discover_manifest_url(chain_id).await
            }
        }
    }

    fn validate_manifest_chain(&self, manifest: &SnapshotManifest) -> Result<()> {
        let chain_id = self.env.chain.chain().id();
        eyre::ensure!(
            manifest.chain_id == chain_id,
            "Snapshot chain ID {} does not match selected chain ID {chain_id}",
            manifest.chain_id
        );
        Ok(())
    }
}

/// Resolves explicit `--with-*` / `--with-*-since` / `--with-*-distance` flags
/// into a component selection.
fn explicit_component_selection(
    all: bool,
    since: Option<u64>,
    distance: Option<u64>,
) -> Option<ComponentSelection> {
    if all {
        Some(ComponentSelection::All)
    } else if let Some(block) = since {
        Some(ComponentSelection::Since(block))
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

/// Removes existing snapshot data managed by `reth download` without following symlinks.
fn clear_existing_datadir(target_dir: &Path, static_files_dir: Option<&Path>) -> Result<()> {
    info!(target: "reth::cli", dir = ?target_dir, "Clearing existing snapshot data");
    for entry in MANAGED_DATADIR_PATHS {
        let path = managed_datadir_path(entry, target_dir, static_files_dir);
        let file_type = match path.symlink_metadata() {
            Ok(metadata) => metadata.file_type(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(fs::FsPathError::metadata(err, path).into()),
        };
        let is_dir = file_type.is_dir();
        #[cfg(windows)]
        let is_dir = is_dir || std::os::windows::fs::FileTypeExt::is_symlink_dir(&file_type);
        if is_dir {
            fs::remove_dir_all(&path)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            fs::remove_file(&path)?;
        }
    }

    Ok(())
}

/// Whether an archive entry belongs to the downloader's managed output paths.
fn is_managed_snapshot_path(path: &Path, directory: bool) -> bool {
    let mut components = path.components().peekable();
    if components.peek() == Some(&Component::CurDir) {
        components.next();
    }
    let Some(Component::Normal(root)) = components.next() else { return false };
    if !MANAGED_DATADIR_PATHS.iter().any(|name| root == *name) {
        return false
    }
    if root == "reth.toml" {
        return !directory && components.next().is_none()
    }
    (directory || components.peek().is_some()) &&
        components.all(|part| matches!(part, Component::Normal(_)))
}

/// Resolves one of [`MANAGED_DATADIR_PATHS`], honoring a custom static files directory.
fn managed_datadir_path(
    entry: &str,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
) -> PathBuf {
    if entry == "static_files" {
        static_files_dir.map_or_else(|| target_dir.join(entry), Path::to_path_buf)
    } else {
        target_dir.join(entry)
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

/// Selects the manifest's RocksDB indices, if it publishes them.
fn inject_rocksdb_indices(
    selections: &mut BTreeMap<SnapshotComponentType, ComponentSelection>,
    manifest: &SnapshotManifest,
) {
    if manifest.component(SnapshotComponentType::RocksdbIndices).is_some() {
        selections.insert(SnapshotComponentType::RocksdbIndices, ComponentSelection::All);
    }
}

/// Returns whether the download omits the RocksDB index files.
fn rocksdb_indices_missing(
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

fn download_command() -> String {
    download_command_for_binary(&current_binary_name())
}

fn download_command_for_binary(binary_name: &str) -> String {
    format!("{binary_name} download")
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
    /// Returns a reference to the environment arguments.
    pub const fn env(&self) -> &EnvironmentArgs<C> {
        &self.env
    }

    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }

    /// Returns whether this command should print its modular archive plan and exit.
    pub const fn prints_plan_json(&self) -> bool {
        self.print_plan_json
    }
}

/// A modular snapshot download after manifest and data-directory resolution.
#[derive(Debug)]
pub struct PreparedSnapshotDownload {
    /// Manifest selected by the command, with a normalized `base_url`.
    pub manifest: SnapshotManifest,
    /// Chain-resolved directory where Reth installs the snapshot.
    pub data_dir: PathBuf,
}

struct ResolvedDownload {
    manifest: SnapshotManifest,
    selections: BTreeMap<SnapshotComponentType, ComponentSelection>,
    preset: Option<SelectionPreset>,
    planned: PlannedDownloads,
}

const MAX_DOWNLOAD_RETRIES: u32 = 10;
const RETRY_BACKOFF_SECS: u64 = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};
    use extract::CompressionFormat;
    use manifest::{generate_manifest, ComponentManifest, OutputFileChecksum, SingleArchive};
    use reth_chainspec::{HOLESKY, MAINNET};
    use reth_db::{
        mdbx::DatabaseArguments, models::StorageSettings, static_file::iter_static_files,
        DatabaseEnv,
    };
    use reth_db_api::{tables, transaction::DbTxMut};
    use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
    use reth_prune_types::{PruneCheckpoint, PruneSegment};
    use reth_stages_types::{StageCheckpoint, StageId};
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::metadata::keys;
    use tempfile::{tempdir, TempDir};

    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    pub(super) fn empty_manifest() -> SnapshotManifest {
        SnapshotManifest {
            block: 0,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: None,
            reth_version: None,
            components: BTreeMap::new(),
            extensions: Default::default(),
        }
    }

    fn single_archive(
        component: SnapshotComponentType,
        output_path: &str,
    ) -> (String, ComponentManifest) {
        let key = component.key();
        let archive = SingleArchive {
            file: format!("{key}.tar.zst"),
            size: 1,
            decompressed_size: 1,
            blake3: None,
            output_files: vec![OutputFileChecksum {
                path: output_path.to_owned(),
                size: 1,
                blake3: blake3::hash(b"x").to_hex().to_string(),
            }],
        };
        (key.to_owned(), ComponentManifest::Single(archive))
    }

    fn manifest_with_archive_only_components() -> SnapshotManifest {
        SnapshotManifest {
            base_url: Some("https://example.com".to_string()),
            components: BTreeMap::from([
                single_archive(
                    SnapshotComponentType::TransactionSenders,
                    "static_files/static_file_transaction_senders_0_0",
                ),
                single_archive(SnapshotComponentType::RocksdbIndices, "rocksdb/000001.sst"),
            ]),
            ..empty_manifest()
        }
    }

    #[cfg(unix)]
    #[test]
    fn clear_existing_datadir_removes_managed_links_without_following_them() {
        let dir = tempfile::tempdir().unwrap();
        let datadir = dir.path().join("datadir");
        let existing = dir.path().join("storage");
        let missing = dir.path().join("old-config.toml");
        fs::create_dir_all(&datadir).unwrap();
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("data"), b"keep").unwrap();
        std::os::unix::fs::symlink(&existing, datadir.join("db")).unwrap();
        std::os::unix::fs::symlink(&missing, datadir.join("reth.toml")).unwrap();

        clear_existing_datadir(&datadir, None).unwrap();

        for entry in ["db", "reth.toml"] {
            let err = datadir.join(entry).symlink_metadata().unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        }
        assert_eq!(fs::read(existing.join("data")).unwrap(), b"keep");
        assert!(!missing.exists());
    }

    #[test]
    fn managed_snapshot_paths_match_output_types() {
        assert!(is_managed_snapshot_path(Path::new("./db/data"), false));
        assert!(is_managed_snapshot_path(Path::new("static_files"), true));
        assert!(is_managed_snapshot_path(Path::new("reth.toml"), false));
        assert!(!is_managed_snapshot_path(Path::new("db"), false));
        assert!(!is_managed_snapshot_path(Path::new("reth.toml/settings"), false));
        assert!(!is_managed_snapshot_path(Path::new("reth.toml"), true));
        assert!(!is_managed_snapshot_path(Path::new("logs/restore.log"), false));
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
        let defaults = DownloadDefaults::default()
            .with_snapshot_api_url("https://snapshots.tempoxyz.dev/api/snapshots");
        let help = defaults.long_help();

        assert_eq!(
            defaults.default_chain_aware_base_url.as_deref(),
            Some("https://snapshots.tempoxyz.dev")
        );
        assert!(help.contains("Browse available snapshots at https://snapshots.tempoxyz.dev"));
        assert!(help.contains("- https://snapshots.tempoxyz.dev (default)"));
        assert!(help.contains("selected chain"));
        assert!(!help.contains("Ethereum mainnet"));
        assert!(!help.contains("snapshots.reth.rs"));
    }

    #[test]
    fn test_snapshot_source_url_sets_generated_references() {
        let defaults =
            DownloadDefaults::default().with_snapshot_source_url("https://snapshots.tempoxyz.dev/");
        let help = defaults.long_help();

        assert_eq!(defaults.snapshot_api_url, "https://snapshots.tempoxyz.dev/api/snapshots");
        assert_eq!(defaults.default_base_url, "https://snapshots.tempoxyz.dev");
        assert_eq!(
            defaults.default_chain_aware_base_url.as_deref(),
            Some("https://snapshots.tempoxyz.dev")
        );
        assert_eq!(
            defaults.available_snapshots.iter().map(|source| source.as_ref()).collect::<Vec<_>>(),
            vec!["https://snapshots.tempoxyz.dev (default)"]
        );
        assert!(!defaults.mainnet_only_discovery());
        assert!(help.contains("Browse available snapshots at https://snapshots.tempoxyz.dev"));
        assert!(help.contains("from https://snapshots.tempoxyz.dev."));
    }

    #[test]
    fn test_snapshot_api_url_trailing_slash_sets_source_url() {
        let defaults = DownloadDefaults::default()
            .with_snapshot_api_url("https://snapshots.tempoxyz.dev/api/snapshots/");
        let help = defaults.long_help();

        assert_eq!(
            defaults.default_chain_aware_base_url.as_deref(),
            Some("https://snapshots.tempoxyz.dev")
        );
        assert!(help.contains("Browse available snapshots at https://snapshots.tempoxyz.dev"));
        assert!(help.contains("- https://snapshots.tempoxyz.dev (default)"));
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
    fn test_download_prune_unlisted_conflicts_with_force_list_and_url() {
        let parse = |args: &[&str]| {
            CommandParser::<DownloadCommand<EthereumChainSpecParser>>::try_parse_from(args)
        };

        assert!(parse(&["reth", "--prune-unlisted"]).unwrap().args.prune_unlisted);
        assert!(parse(&["reth", "--prune-unlisted", "--force"]).is_err());
        assert!(parse(&["reth", "--prune-unlisted", "--list"]).is_err());
        assert!(
            parse(&["reth", "--prune-unlisted", "--url", "https://example.com/a.tar.zst"]).is_err()
        );
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
    fn test_download_print_plan_json_parses() {
        let args = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from([
            "reth",
            "--manifest-path",
            "manifest.json",
            "--minimal",
            "--print-plan-json",
        ])
        .args;

        assert!(args.prints_plan_json());
    }

    #[test]
    fn test_download_print_plan_json_rejects_single_archive() {
        let result = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::try_parse_from([
            "reth",
            "--url",
            "https://example.com/snapshot.tar.zst",
            "--print-plan-json",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn minimal_component_selection_uses_configured_prune_modes() {
        let args =
            CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from(["reth"]).args;
        let history_distance = 64_864;
        let modes = PruneModes {
            sender_recovery: Some(PruneMode::Full),
            receipts: Some(PruneMode::Distance(128)),
            account_history: Some(PruneMode::Distance(history_distance)),
            storage_history: Some(PruneMode::Distance(history_distance)),
            bodies_history: Some(PruneMode::Distance(history_distance)),
            ..Default::default()
        };

        assert_eq!(
            args.pruning_selection_for_component(
                SnapshotComponentType::Transactions,
                1_000_000,
                &modes,
                false,
            ),
            ComponentSelection::Distance(history_distance)
        );
        assert_eq!(
            args.pruning_selection_for_component(
                SnapshotComponentType::Receipts,
                1_000_000,
                &modes,
                false,
            ),
            ComponentSelection::Distance(128)
        );
        assert_eq!(
            args.pruning_selection_for_component(
                SnapshotComponentType::AccountChangesets,
                1_000_000,
                &modes,
                false,
            ),
            ComponentSelection::Distance(history_distance)
        );
        assert_eq!(
            args.pruning_selection_for_component(
                SnapshotComponentType::StorageChangesets,
                1_000_000,
                &modes,
                false,
            ),
            ComponentSelection::Distance(history_distance)
        );
        assert_eq!(
            args.pruning_selection_for_component(
                SnapshotComponentType::TransactionSenders,
                1_000_000,
                &modes,
                false,
            ),
            ComponentSelection::None
        );
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
    fn inject_rocksdb_indices_keeps_index_stages() {
        let manifest = manifest_with_archive_only_components();
        let mut selections = BTreeMap::new();
        selections
            .insert(SnapshotComponentType::AccountChangesets, ComponentSelection::Distance(10));

        inject_rocksdb_indices(&mut selections, &manifest);

        assert_eq!(
            selections.get(&SnapshotComponentType::RocksdbIndices),
            Some(&ComponentSelection::All)
        );
        assert_eq!(selections.get(&SnapshotComponentType::TransactionSenders), None);
    }

    #[test]
    fn inject_rocksdb_indices_skips_manifest_without_indices() {
        let mut manifest = manifest_with_archive_only_components();
        manifest.components.clear();
        let mut selections = BTreeMap::new();

        inject_rocksdb_indices(&mut selections, &manifest);

        assert!(selections.is_empty());
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

    #[test]
    fn download_command_uses_binary_name() {
        assert_eq!(download_command_for_binary("tempo"), "tempo download");
    }

    #[tokio::test]
    async fn restore_rejects_incompatible_layout_at_finalization() -> Result<()> {
        let source = tempdir()?;
        snapshot_db(&source)?.update(|tx| {
            set_layout(tx, serde_json::json!({"account_changesets_in_static_files": true}));
        })?;
        let error = restore_history(&source, "--archive", None).await.unwrap_err();
        assert_eq!(error.to_string(), "Unsupported legacy snapshot setting for this download: account_changesets_in_static_files=true");
        Ok(())
    }

    #[tokio::test]
    async fn test_download_explicit_source_and_datadir_allow_other_chains() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.json");
        let mut manifest = manifest_with_archive_only_components();
        manifest.chain_id = 42431;
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let manifest_url = format!("file://{}", manifest_path.display());
        let target = dir.path().join("download");

        for source in ["--manifest-url", "--manifest-path"] {
            let location = if source == "--manifest-url" {
                manifest_url.as_str()
            } else {
                manifest_path.to_str().unwrap()
            };
            for selection in ["--archive", "--minimal", "--non-interactive", "--with-txs"] {
                let args = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from([
                    "reth",
                    source,
                    location,
                    "--datadir",
                    target.to_str().unwrap(),
                    selection,
                ])
                .args;
                let (plan, prepared) = args.plan().await.unwrap();
                assert_eq!(plan.chain_id, 42431);
                assert_eq!(prepared.manifest.chain_id, 42431);
                assert_eq!(prepared.data_dir, target);
                assert!(!target.exists());
            }
        }
    }

    #[tokio::test]
    async fn test_download_chain_derived_datadir_rejects_other_chains() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.json");
        let mut manifest = manifest_with_archive_only_components();
        manifest.chain_id = 42431;
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        for datadir in [vec![], vec!["--datadir", "default"]] {
            let mut cli_args =
                vec!["reth", "--manifest-path", manifest_path.to_str().unwrap(), "--archive"];
            cli_args.extend(datadir);
            let args =
                CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from(cli_args)
                    .args;
            assert_eq!(
                args.plan().await.unwrap_err().to_string(),
                "Snapshot chain ID 42431 does not match selected chain ID 1"
            );
        }
    }

    #[test]
    fn test_download_full_chain_derived_pruning_rejects_other_chains() {
        let args = CommandParser::<DownloadCommand<EthereumChainSpecParser>>::parse_from([
            "reth",
            "--datadir",
            "download",
            "--manifest-url",
            "https://example.com/manifest.json",
            "--full",
        ])
        .args;
        let mut manifest = manifest_with_archive_only_components();
        manifest.chain_id = 42431;
        assert_eq!(
            args.full_preset_selections(&manifest).unwrap_err().to_string(),
            "Snapshot chain ID 42431 does not match selected chain ID 1"
        );
        manifest.chain_id = 1;
        assert!(args.full_preset_selections(&manifest).is_ok());
    }

    const SNAPSHOT_TIP: u64 = 10;

    const INDEX_STAGES: [StageId; 3] =
        [StageId::TransactionLookup, StageId::IndexAccountHistory, StageId::IndexStorageHistory];

    const SOURCE_BOUNDARY: PruneCheckpoint = PruneCheckpoint::pruned_through(3);
    const SOURCE_HISTORY: PruneCheckpoint =
        PruneCheckpoint { prune_mode: PruneMode::Distance(30_000), ..SOURCE_BOUNDARY };

    fn snapshot_db(dir: &TempDir) -> Result<DatabaseEnv> {
        init_db(dir.path().join("db"), DatabaseArguments::test())
    }

    pub(super) fn history_checkpoints(tx: &impl DbTx) -> Result<[Option<PruneCheckpoint>; 2]> {
        Ok([
            tx.get::<tables::PruneCheckpoints>(PruneSegment::AccountHistory)?,
            tx.get::<tables::PruneCheckpoints>(PruneSegment::StorageHistory)?,
        ])
    }

    pub(super) fn set_history_checkpoints(
        tx: &impl DbTxMut,
        account: PruneCheckpoint,
        storage: PruneCheckpoint,
    ) {
        tx.put::<tables::PruneCheckpoints>(PruneSegment::AccountHistory, account).unwrap();
        tx.put::<tables::PruneCheckpoints>(PruneSegment::StorageHistory, storage).unwrap();
    }

    async fn restore_history(
        source: &TempDir,
        flag: &str,
        expected_mode: Option<PruneMode>,
    ) -> Result<TempDir> {
        let target = tempdir()?;
        restore_snapshot(source, &target, flag).await?;
        let modes = Config::from_path(target.path().join("reth.toml"))?.prune.segments;
        assert_eq!(modes.account_history, expected_mode);
        assert_eq!(modes.storage_history, expected_mode);
        Ok(target)
    }

    async fn restore_snapshot(source: &TempDir, target: &TempDir, flag: &str) -> Result<()> {
        let archives = tempdir()?;
        let output = archives.path();
        let manifest =
            generate_manifest(source.path(), output, None, SNAPSHOT_TIP, 1, 10, None, false)?;
        let path = output.join("manifest.json");
        fs::write(&path, serde_json::to_vec(&manifest)?)?;
        DownloadCommand::<EthereumChainSpecParser>::parse_from([
            "reth",
            "--manifest-path",
            path.to_str().unwrap(),
            "--datadir",
            target.path().to_str().unwrap(),
            flag,
        ])
        .execute::<()>()
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn restore_bundled_v1_history() -> Result<()> {
        for (setting, flag, mode) in [
            ("receipts_in_static_files", "--with-state-history", None),
            (
                "transaction_senders_in_static_files",
                "--minimal",
                Some(PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE)),
            ),
        ] {
            let source = tempdir()?;
            let indexed = StageCheckpoint::new(SNAPSHOT_TIP);
            {
                let db = snapshot_db(&source)?;
                let tx = db.tx_mut()?;
                // v1.10 configured static-file receipts and senders independently of its history.
                set_layout(&tx, serde_json::json!({(setting): true}));
                for stage in INDEX_STAGES {
                    tx.put::<tables::StageCheckpoints>(stage.to_string(), indexed)?;
                    tx.put::<tables::StageCheckpointProgresses>(stage.to_string(), vec![1, 2, 3])?;
                }
                for segment in [PruneSegment::StorageHistory, PruneSegment::TransactionLookup] {
                    tx.put::<tables::PruneCheckpoints>(segment, SOURCE_HISTORY)?;
                }
                tx.commit()?;
            }
            let target = restore_history(&source, flag, mode).await?;
            let db = snapshot_db(&target)?;
            let tx = db.tx()?;
            assert_eq!(history_checkpoints(&tx)?, [None, Some(SOURCE_HISTORY)]);
            assert_eq!(
                tx.get::<tables::PruneCheckpoints>(PruneSegment::TransactionLookup)?,
                Some(SOURCE_HISTORY)
            );
            for stage in INDEX_STAGES {
                assert_eq!(tx.get::<tables::StageCheckpoints>(stage.to_string())?, Some(indexed));
                assert_eq!(
                    tx.get::<tables::StageCheckpointProgresses>(stage.to_string())?,
                    Some(vec![1, 2, 3])
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn future_history_keeps_v1_checkpoint_progress() -> Result<()> {
        let source = tempdir()?;
        let interrupted = PruneCheckpoint {
            block_number: Some(4),
            tx_number: None,
            prune_mode: PruneMode::Before(SNAPSHOT_TIP),
        };
        let unstarted = PruneCheckpoint { block_number: None, ..SOURCE_HISTORY };
        snapshot_db(&source)?.update(|tx| set_history_checkpoints(tx, interrupted, unstarted))?;
        let mode = PruneMode::Before(20);
        let target = restore_history(&source, "--with-state-history-since=20", Some(mode)).await?;
        assert_eq!(
            history_checkpoints(&snapshot_db(&target)?.tx()?)?,
            [Some(interrupted), Some(unstarted)]
        );
        Ok(())
    }

    #[tokio::test]
    async fn restore_v2_history_uses_selected_range_and_retention() -> Result<()> {
        let source = tempdir()?;
        snapshot_db(&source)?.update(|tx| {
            set_layout(tx, StorageSettings::v2());
            set_history_checkpoints(tx, SOURCE_HISTORY, SOURCE_HISTORY);
        })?;
        for segment in [StaticFileSegment::AccountChangeSets, StaticFileSegment::StorageChangeSets]
        {
            manifest::tests::write_test_static_files(source.path(), segment, SNAPSHOT_TIP);
        }
        let future = PruneMode::Before(20);
        let tip = PruneCheckpoint::pruned_through(SNAPSHOT_TIP);
        for (flag, mode, checkpoint, segment_count) in [
            (
                "--with-state-history-since=20",
                Some(future),
                PruneCheckpoint { prune_mode: future, ..tip },
                0,
            ),
            ("--with-state-history", None, SOURCE_BOUNDARY, 2),
        ] {
            let target = restore_history(&source, flag, mode).await?;
            assert_eq!(
                iter_static_files(&target.path().join("static_files"))?.len(),
                segment_count,
                "{flag}"
            );
            let db = snapshot_db(&target)?;
            let tx = db.tx()?;
            assert_eq!(history_checkpoints(&tx)?, [Some(checkpoint); 2]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn restore_v2_without_history_or_indices() -> Result<()> {
        let source = tempdir()?;
        let indexed = StageCheckpoint::new(SNAPSHOT_TIP);
        let tip = PruneCheckpoint::pruned_through(SNAPSHOT_TIP);
        let source_prune = PruneCheckpoint { prune_mode: PruneMode::Full, ..tip };
        {
            let db = snapshot_db(&source)?;
            let tx = db.tx_mut()?;
            set_layout(&tx, StorageSettings::v2());
            for stage in INDEX_STAGES.into_iter().chain([StageId::SenderRecovery]) {
                tx.put::<tables::StageCheckpoints>(stage.to_string(), indexed)?;
            }
            for segment in [PruneSegment::TransactionLookup, PruneSegment::SenderRecovery] {
                tx.put::<tables::PruneCheckpoints>(segment, source_prune)?;
            }
            tx.commit()?;
        }
        let target = restore_history(&source, "--with-state-history", None).await?;
        let db = snapshot_db(&target)?;
        let tx = db.tx()?;
        assert_eq!(history_checkpoints(&tx)?, [Some(tip); 2]);
        for stage in INDEX_STAGES {
            assert_eq!(
                tx.get::<tables::StageCheckpoints>(stage.to_string())?,
                Some(StageCheckpoint::default()),
                "{stage}"
            );
        }
        assert_eq!(tx.get::<tables::PruneCheckpoints>(PruneSegment::TransactionLookup)?, None);
        assert_eq!(
            tx.get::<tables::StageCheckpoints>(StageId::SenderRecovery.to_string())?,
            Some(indexed)
        );
        assert_eq!(
            tx.get::<tables::PruneCheckpoints>(PruneSegment::SenderRecovery)?,
            Some(source_prune)
        );
        Ok(())
    }

    #[tokio::test]
    async fn minimal_restore_with_existing_archive_config() -> Result<()> {
        let source = tempdir()?;
        snapshot_db(&source)?.update(|tx| set_layout(tx, StorageSettings::v2()))?;
        let target = tempdir()?;
        let config_path = target.path().join("reth.toml");
        let archive_config = "[prune.segments]\n";
        fs::write(&config_path, archive_config)?;

        restore_snapshot(&source, &target, "--minimal").await?;

        assert_eq!(fs::read_to_string(config_path)?, archive_config);
        let db = snapshot_db(&target)?;
        let tx = db.tx()?;
        for (segment, prune_mode) in [
            (PruneSegment::SenderRecovery, PruneMode::Full),
            (PruneSegment::Bodies, PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE)),
        ] {
            assert_eq!(
                tx.get::<tables::PruneCheckpoints>(segment)?,
                Some(PruneCheckpoint {
                    prune_mode,
                    ..PruneCheckpoint::pruned_through(SNAPSHOT_TIP)
                }),
                "{segment}"
            );
        }
        Ok(())
    }

    pub(super) fn set_layout(tx: &impl DbTxMut, settings: impl serde::Serialize) {
        let bytes = serde_json::to_vec(&settings).unwrap();
        tx.put::<tables::Metadata>(keys::STORAGE_SETTINGS.to_owned(), bytes).unwrap();
    }
}
