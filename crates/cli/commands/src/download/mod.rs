pub mod config_gen;
pub mod manifest;
pub mod manifest_cmd;
mod tui;

use crate::common::EnvironmentArgs;
use blake3::Hasher;
use clap::Parser;
use config_gen::{config_for_selections, write_config};
use eyre::{Result, WrapErr};
use futures::stream::{self, StreamExt};
use lz4::Decoder;
use manifest::{
    ArchiveDescriptor, ComponentSelection, OutputFileChecksum, SnapshotComponentType,
    SnapshotManifest,
};
use reqwest::{blocking::Client as BlockingClient, header::RANGE, Client, StatusCode};
use reth_chainspec::{EthChainSpec, EthereumHardfork, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_util::cancellation::CancellationToken;
use reth_db::{init_db, Database};
use reth_db_api::transaction::DbTx;
use reth_fs_util as fs;
use reth_node_core::args::DefaultPruningValues;
use reth_prune_types::PruneMode;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use tar::Archive;
use tokio::task;
use tracing::{info, warn};
use tui::{run_selector, SelectorOutput};
use url::Url;
use zstd::stream::read::Decoder as ZstdDecoder;

const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const RETH_SNAPSHOTS_BASE_URL: &str = "https://snapshots-r2.reth.rs";
const RETH_SNAPSHOTS_API_URL: &str = "https://snapshots.reth.rs/api/snapshots";
const EXTENSION_TAR_LZ4: &str = ".tar.lz4";
const EXTENSION_TAR_ZSTD: &str = ".tar.zst";
const DOWNLOAD_CACHE_DIR: &str = ".download-cache";

/// Maximum number of concurrent archive downloads.
const MAX_CONCURRENT_DOWNLOADS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionPreset {
    Minimal,
    Full,
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

        let mut help = String::from(
            "Specify a snapshot URL or let the command propose a default one.\n\n\
             Browse available snapshots at https://snapshots.reth.rs\n\
             or use --list-snapshots to see them from the CLI.\n\nAvailable snapshot sources:\n",
        );

        for source in &self.available_snapshots {
            help.push_str("- ");
            help.push_str(source);
            help.push('\n');
        }

        help.push_str(
            "\nIf no URL is provided, the latest archive snapshot for the selected chain\nwill be proposed for download from ",
        );
        help.push_str(
            self.default_chain_aware_base_url.as_deref().unwrap_or(&self.default_base_url),
        );
        help.push_str(
            ".\n\nLocal file:// URLs are also supported for extracting snapshots from disk.",
        );
        help
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

    /// Builder: Set custom long help text, overriding the generated help
    pub fn with_long_help(mut self, help: impl Into<String>) -> Self {
        self.long_help = Some(help.into());
        self
    }
}

impl Default for DownloadDefaults {
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
    /// Browse available snapshots at <https://snapshots.reth.rs> or use --list-snapshots.
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

    /// Include transaction static files.
    #[arg(long, conflicts_with_all = ["minimal", "full", "archive"])]
    with_txs: bool,

    /// Include receipt static files.
    #[arg(long, conflicts_with_all = ["minimal", "full", "archive"])]
    with_receipts: bool,

    /// Include account and storage history static files.
    #[arg(long, alias = "with-changesets", conflicts_with_all = ["minimal", "full", "archive"])]
    with_state_history: bool,

    /// Include transaction sender static files. Requires `--with-txs`.
    #[arg(long, requires = "with_txs", conflicts_with_all = ["minimal", "full", "archive"])]
    with_senders: bool,

    /// Include RocksDB index files.
    #[arg(long, conflicts_with_all = ["minimal", "full", "archive", "without_rocksdb"])]
    with_rocksdb: bool,

    /// Download all available components (archive node, no pruning).
    #[arg(long, alias = "all", conflicts_with_all = ["with_txs", "with_receipts", "with_state_history", "with_senders", "with_rocksdb", "minimal", "full"])]
    archive: bool,

    /// Download the minimal component set (same default as --non-interactive).
    #[arg(long, conflicts_with_all = ["with_txs", "with_receipts", "with_state_history", "with_senders", "with_rocksdb", "archive", "full"])]
    minimal: bool,

    /// Download the full node component set (matches default full prune settings).
    #[arg(long, conflicts_with_all = ["with_txs", "with_receipts", "with_state_history", "with_senders", "with_rocksdb", "archive", "minimal"])]
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

    /// Use resumable two-phase downloads (download to disk first, then extract).
    ///
    /// Archives are downloaded to a .part file with HTTP Range resume support
    /// before extraction. Slower but tolerates network interruptions without
    /// restarting. By default, archives stream directly into the extractor.
    #[arg(long)]
    resumable: bool,

    /// Maximum number of concurrent modular archive workers.
    #[arg(long, default_value_t = MAX_CONCURRENT_DOWNLOADS)]
    download_concurrency: usize,

    /// List available snapshots from snapshots.reth.rs and exit.
    ///
    /// Queries the snapshots API and prints all available snapshots for the selected chain,
    /// including block number, size, and manifest URL.
    #[arg(long, alias = "list-snapshots", conflicts_with_all = ["url", "manifest_url", "manifest_path"])]
    list: bool,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> DownloadCommand<C> {
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
                cancel_token.clone(),
            )
            .await?;
            info!(target: "reth::cli", "Snapshot downloaded and extracted successfully");

            return Ok(());
        }

        // Modular download: fetch manifest and select components
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

        let ResolvedComponents { mut selections, preset } = self.resolve_components(&manifest)?;

        if matches!(preset, Some(SelectionPreset::Archive)) {
            inject_archive_only_components(&mut selections, &manifest, !self.without_rocksdb);
        }

        // Collect all archive descriptors across selected components.
        let target_dir = data_dir.data_dir();
        let mut all_downloads: Vec<PlannedArchive> = Vec::new();
        for (ty, sel) in &selections {
            let distance = match sel {
                ComponentSelection::All => None,
                ComponentSelection::Distance(d) => Some(*d),
                ComponentSelection::None => continue,
            };
            let descriptors = manifest.archive_descriptors_for_distance(*ty, distance);
            let name = ty.display_name().to_string();

            if !descriptors.is_empty() {
                info!(target: "reth::cli",
                    component = %name,
                    archives = descriptors.len(),
                    selection = %sel,
                    "Queued component for download"
                );
            }

            for descriptor in descriptors {
                if descriptor.output_files.is_empty() {
                    eyre::bail!(
                        "Invalid modular manifest: {} is missing plain output checksum metadata",
                        descriptor.file_name
                    );
                }
                all_downloads.push(PlannedArchive {
                    ty: *ty,
                    component: name.clone(),
                    archive: descriptor,
                });
            }
        }

        all_downloads.sort_by(|a, b| {
            archive_priority_rank(a.ty)
                .cmp(&archive_priority_rank(b.ty))
                .then_with(|| a.component.cmp(&b.component))
                .then_with(|| a.archive.file_name.cmp(&b.archive.file_name))
        });

        let download_cache_dir = if self.resumable {
            let dir = target_dir.join(DOWNLOAD_CACHE_DIR);
            fs::create_dir_all(&dir)?;
            Some(dir)
        } else {
            None
        };

        let total_archives = all_downloads.len();
        let total_size: u64 = selections
            .iter()
            .map(|(ty, sel)| match sel {
                ComponentSelection::All => manifest.size_for_distance(*ty, None),
                ComponentSelection::Distance(d) => manifest.size_for_distance(*ty, Some(*d)),
                ComponentSelection::None => 0,
            })
            .sum();

        let startup_summary = summarize_download_startup(&all_downloads, target_dir)?;
        info!(target: "reth::cli",
            reusable = startup_summary.reusable,
            needs_download = startup_summary.needs_download,
            "Startup integrity summary (plain output files)"
        );

        info!(target: "reth::cli",
            archives = total_archives,
            total = %DownloadProgress::format_size(total_size),
            "Downloading all archives"
        );

        let shared = SharedProgress::new(total_size, total_archives as u64, cancel_token.clone());
        let progress_handle = spawn_progress_display(Arc::clone(&shared));

        let target = target_dir.to_path_buf();
        let cache_dir = download_cache_dir;
        let resumable = self.resumable;
        let download_concurrency = self.download_concurrency.max(1);
        let results: Vec<Result<()>> = stream::iter(all_downloads)
            .map(|planned| {
                let dir = target.clone();
                let cache = cache_dir.clone();
                let sp = Arc::clone(&shared);
                let ct = cancel_token.clone();
                async move {
                    process_modular_archive(
                        planned,
                        &dir,
                        cache.as_deref(),
                        Some(sp),
                        resumable,
                        ct,
                    )
                    .await?;
                    Ok(())
                }
            })
            .buffer_unordered(download_concurrency)
            .collect()
            .await;

        shared.done.store(true, Ordering::Relaxed);
        let _ = progress_handle.await;

        // Check for errors
        for result in results {
            result?;
        }

        // Generate reth.toml and set prune checkpoints
        let config =
            config_for_selections(&selections, &manifest, preset, Some(self.env.chain.as_ref()));
        if write_config(&config, target_dir)? {
            let desc = config_gen::describe_prune_config(&config);
            info!(target: "reth::cli", "{}", desc.join(", "));
        }

        // Open the DB to write checkpoints
        let db_path = data_dir.db();
        let db = init_db(&db_path, self.env.db.database_args())?;

        // Write prune checkpoints to the DB so the pruner knows data before the
        // snapshot block is already in the expected pruned state
        let should_write_prune = config.prune.segments != Default::default();
        let should_reset_indices = should_reset_index_stage_checkpoints(&selections);
        if should_write_prune || should_reset_indices {
            let tx = db.tx_mut()?;

            if should_write_prune {
                config_gen::write_prune_checkpoints_tx(&tx, &config, manifest.block)?;
            }

            // Reset stage checkpoints for history indexing stages only if RocksDB
            // indices weren't downloaded. When archive snapshots include the
            // optional RocksDB indices component, we preserve source checkpoints.
            if should_reset_indices {
                config_gen::reset_index_stage_checkpoints_tx(&tx)?;
            }

            tx.commit()?;
        }

        info!(target: "reth::cli", "Snapshot download complete. Run `reth node` to start syncing.");

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
            self.with_receipts ||
            self.with_state_history ||
            self.with_senders ||
            self.with_rocksdb;

        if has_explicit_flags {
            let mut selections = BTreeMap::new();
            // Required components always All
            if available(SnapshotComponentType::State) {
                selections.insert(SnapshotComponentType::State, ComponentSelection::All);
            }
            if available(SnapshotComponentType::Headers) {
                selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
            }
            if self.with_txs && available(SnapshotComponentType::Transactions) {
                selections.insert(SnapshotComponentType::Transactions, ComponentSelection::All);
            }
            if self.with_receipts && available(SnapshotComponentType::Receipts) {
                selections.insert(SnapshotComponentType::Receipts, ComponentSelection::All);
            }
            if self.with_state_history {
                if available(SnapshotComponentType::AccountChangesets) {
                    selections
                        .insert(SnapshotComponentType::AccountChangesets, ComponentSelection::All);
                }
                if available(SnapshotComponentType::StorageChangesets) {
                    selections
                        .insert(SnapshotComponentType::StorageChangesets, ComponentSelection::All);
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
                        Some(paris) if snapshot_block >= paris => {
                            ComponentSelection::Distance(snapshot_block - paris + 1)
                        }
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

    async fn resolve_manifest_source(&self, chain_id: u64) -> Result<String> {
        if let Some(path) = &self.manifest_path {
            return Ok(path.display().to_string());
        }

        match &self.manifest_url {
            Some(url) => Ok(url.clone()),
            None => discover_manifest_url(chain_id).await,
        }
    }
}

fn selection_from_prune_mode(mode: Option<PruneMode>, snapshot_block: u64) -> ComponentSelection {
    match mode {
        None => ComponentSelection::All,
        Some(PruneMode::Full) => ComponentSelection::None,
        Some(PruneMode::Distance(d)) => ComponentSelection::Distance(d),
        Some(PruneMode::Before(block)) => {
            if snapshot_block >= block {
                ComponentSelection::Distance(snapshot_block - block + 1)
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

fn should_reset_index_stage_checkpoints(
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> bool {
    !matches!(selections.get(&SnapshotComponentType::RocksdbIndices), Some(ComponentSelection::All))
}

impl<C: ChainSpecParser> DownloadCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

/// Tracks download progress and throttles display updates to every 100ms.
pub(crate) struct DownloadProgress {
    downloaded: u64,
    total_size: u64,
    last_displayed: Instant,
    started_at: Instant,
}

#[derive(Debug, Clone)]
struct PlannedArchive {
    ty: SnapshotComponentType,
    component: String,
    archive: ArchiveDescriptor,
}

const fn archive_priority_rank(ty: SnapshotComponentType) -> u8 {
    match ty {
        SnapshotComponentType::State => 0,
        SnapshotComponentType::RocksdbIndices => 1,
        _ => 2,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct DownloadStartupSummary {
    reusable: usize,
    needs_download: usize,
}

fn summarize_download_startup(
    all_downloads: &[PlannedArchive],
    target_dir: &Path,
) -> Result<DownloadStartupSummary> {
    let mut summary = DownloadStartupSummary::default();

    for planned in all_downloads {
        if verify_output_files(target_dir, &planned.archive.output_files)? {
            summary.reusable += 1;
        } else {
            summary.needs_download += 1;
        }
    }

    Ok(summary)
}

impl DownloadProgress {
    /// Creates new progress tracker with given total size
    fn new(total_size: u64) -> Self {
        let now = Instant::now();
        Self { downloaded: 0, total_size, last_displayed: now, started_at: now }
    }

    /// Converts bytes to human readable format (B, KB, MB, GB)
    pub(crate) fn format_size(size: u64) -> String {
        let mut size = size as f64;
        let mut unit_index = 0;

        while size >= 1024.0 && unit_index < BYTE_UNITS.len() - 1 {
            size /= 1024.0;
            unit_index += 1;
        }

        format!("{:.2} {}", size, BYTE_UNITS[unit_index])
    }

    /// Format duration as human readable string
    fn format_duration(duration: Duration) -> String {
        let secs = duration.as_secs();
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// Updates progress bar (for single-archive legacy downloads)
    fn update(&mut self, chunk_size: u64) -> Result<()> {
        self.downloaded += chunk_size;

        if self.last_displayed.elapsed() >= Duration::from_millis(100) {
            let formatted_downloaded = Self::format_size(self.downloaded);
            let formatted_total = Self::format_size(self.total_size);
            let progress = (self.downloaded as f64 / self.total_size as f64) * 100.0;

            let elapsed = self.started_at.elapsed();
            let eta = if self.downloaded > 0 {
                let remaining = self.total_size.saturating_sub(self.downloaded);
                let speed = self.downloaded as f64 / elapsed.as_secs_f64();
                if speed > 0.0 {
                    Duration::from_secs_f64(remaining as f64 / speed)
                } else {
                    Duration::ZERO
                }
            } else {
                Duration::ZERO
            };
            let eta_str = Self::format_duration(eta);

            print!(
                "\rDownloading and extracting... {progress:.2}% ({formatted_downloaded} / {formatted_total}) ETA: {eta_str}     ",
            );
            io::stdout().flush()?;
            self.last_displayed = Instant::now();
        }

        Ok(())
    }
}

/// Shared progress counter for parallel downloads.
///
/// Each download thread atomically increments `downloaded`. A single display
/// task on the main thread reads the counter periodically and prints one
/// aggregated progress line.
struct SharedProgress {
    downloaded: AtomicU64,
    total_size: u64,
    total_archives: u64,
    archives_done: AtomicU64,
    done: AtomicBool,
    cancel_token: CancellationToken,
}

impl SharedProgress {
    fn new(total_size: u64, total_archives: u64, cancel_token: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            downloaded: AtomicU64::new(0),
            total_size,
            total_archives,
            archives_done: AtomicU64::new(0),
            done: AtomicBool::new(false),
            cancel_token,
        })
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    fn add(&self, bytes: u64) {
        self.downloaded.fetch_add(bytes, Ordering::Relaxed);
    }

    fn archive_done(&self) {
        self.archives_done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Spawns a background task that prints aggregated download progress.
/// Returns a handle; drop it (or call `.abort()`) to stop.
fn spawn_progress_display(progress: Arc<SharedProgress>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let started_at = Instant::now();
        let mut interval = tokio::time::interval(Duration::from_secs(3));
        interval.tick().await; // first tick is immediate, skip it
        loop {
            interval.tick().await;

            if progress.done.load(Ordering::Relaxed) {
                break;
            }

            let downloaded = progress.downloaded.load(Ordering::Relaxed);
            let total = progress.total_size;
            if total == 0 {
                continue;
            }

            let done = progress.archives_done.load(Ordering::Relaxed);
            let all = progress.total_archives;
            let pct = (downloaded as f64 / total as f64) * 100.0;
            let dl = DownloadProgress::format_size(downloaded);
            let tot = DownloadProgress::format_size(total);

            let elapsed = started_at.elapsed();
            let remaining = total.saturating_sub(downloaded);

            if remaining == 0 {
                // Downloads done, waiting for extraction
                info!(target: "reth::cli",
                    archives = format_args!("{done}/{all}"),
                    downloaded = %dl,
                    "Extracting remaining archives"
                );
            } else {
                let eta = if downloaded > 0 {
                    let speed = downloaded as f64 / elapsed.as_secs_f64();
                    if speed > 0.0 {
                        DownloadProgress::format_duration(Duration::from_secs_f64(
                            remaining as f64 / speed,
                        ))
                    } else {
                        "??".to_string()
                    }
                } else {
                    "??".to_string()
                };

                info!(target: "reth::cli",
                    archives = format_args!("{done}/{all}"),
                    progress = format_args!("{pct:.1}%"),
                    downloaded = %dl,
                    total = %tot,
                    eta = %eta,
                    "Downloading"
                );
            }
        }

        // Final line
        let downloaded = progress.downloaded.load(Ordering::Relaxed);
        let dl = DownloadProgress::format_size(downloaded);
        let tot = DownloadProgress::format_size(progress.total_size);
        let elapsed = DownloadProgress::format_duration(started_at.elapsed());
        info!(target: "reth::cli",
            downloaded = %dl,
            total = %tot,
            elapsed = %elapsed,
            "Downloads complete"
        );
    })
}

/// Adapter to track progress while reading (used for extraction in legacy path)
struct ProgressReader<R> {
    reader: R,
    progress: DownloadProgress,
    cancel_token: CancellationToken,
}

impl<R: Read> ProgressReader<R> {
    fn new(reader: R, total_size: u64, cancel_token: CancellationToken) -> Self {
        Self { reader, progress: DownloadProgress::new(total_size), cancel_token }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel_token.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let bytes = self.reader.read(buf)?;
        if bytes > 0 &&
            let Err(e) = self.progress.update(bytes as u64)
        {
            return Err(io::Error::other(e));
        }
        Ok(bytes)
    }
}

/// Supported compression formats for snapshots
#[derive(Debug, Clone, Copy)]
enum CompressionFormat {
    Lz4,
    Zstd,
}

impl CompressionFormat {
    /// Detect compression format from file extension
    fn from_url(url: &str) -> Result<Self> {
        let path =
            Url::parse(url).map(|u| u.path().to_string()).unwrap_or_else(|_| url.to_string());

        if path.ends_with(EXTENSION_TAR_LZ4) {
            Ok(Self::Lz4)
        } else if path.ends_with(EXTENSION_TAR_ZSTD) {
            Ok(Self::Zstd)
        } else {
            Err(eyre::eyre!(
                "Unsupported file format. Expected .tar.lz4 or .tar.zst, got: {}",
                path
            ))
        }
    }
}

/// Extracts a compressed tar archive to the target directory with progress tracking.
fn extract_archive<R: Read>(
    reader: R,
    total_size: u64,
    format: CompressionFormat,
    target_dir: &Path,
    cancel_token: CancellationToken,
) -> Result<()> {
    let progress_reader = ProgressReader::new(reader, total_size, cancel_token);

    match format {
        CompressionFormat::Lz4 => {
            let decoder = Decoder::new(progress_reader)?;
            Archive::new(decoder).unpack(target_dir)?;
        }
        CompressionFormat::Zstd => {
            let decoder = ZstdDecoder::new(progress_reader)?;
            Archive::new(decoder).unpack(target_dir)?;
        }
    }

    println!();
    Ok(())
}

/// Extracts a compressed tar archive without progress tracking.
fn extract_archive_raw<R: Read>(
    reader: R,
    format: CompressionFormat,
    target_dir: &Path,
) -> Result<()> {
    match format {
        CompressionFormat::Lz4 => {
            Archive::new(Decoder::new(reader)?).unpack(target_dir)?;
        }
        CompressionFormat::Zstd => {
            Archive::new(ZstdDecoder::new(reader)?).unpack(target_dir)?;
        }
    }
    Ok(())
}

/// Extracts a snapshot from a local file.
fn extract_from_file(path: &Path, format: CompressionFormat, target_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let total_size = file.metadata()?.len();
    info!(target: "reth::cli",
        file = %path.display(),
        size = %DownloadProgress::format_size(total_size),
        "Extracting local archive"
    );
    let start = Instant::now();
    extract_archive(file, total_size, format, target_dir, CancellationToken::new())?;
    info!(target: "reth::cli",
        file = %path.display(),
        elapsed = %DownloadProgress::format_duration(start.elapsed()),
        "Local extraction complete"
    );
    Ok(())
}

const MAX_DOWNLOAD_RETRIES: u32 = 10;
const RETRY_BACKOFF_SECS: u64 = 5;

/// Wrapper that tracks download progress while writing data.
/// Used with [`io::copy`] to display progress during downloads.
struct ProgressWriter<W> {
    inner: W,
    progress: DownloadProgress,
    cancel_token: CancellationToken,
}

impl<W: Write> Write for ProgressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel_token.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let n = self.inner.write(buf)?;
        let _ = self.progress.update(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Wrapper that bumps a shared atomic counter while writing data.
/// Used for parallel downloads where a single display task shows aggregated progress.
struct SharedProgressWriter<W> {
    inner: W,
    progress: Arc<SharedProgress>,
}

impl<W: Write> Write for SharedProgressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.progress.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let n = self.inner.write(buf)?;
        self.progress.add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Wrapper that bumps a shared atomic counter while reading data.
/// Used for streaming downloads where a single display task shows aggregated progress.
struct SharedProgressReader<R> {
    inner: R,
    progress: Arc<SharedProgress>,
}

impl<R: Read> Read for SharedProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.progress.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.progress.add(n as u64);
        Ok(n)
    }
}

/// Downloads a file with resume support using HTTP Range requests.
/// Automatically retries on failure, resuming from where it left off.
/// Returns the path to the downloaded file and its total size.
///
/// When `shared` is provided, progress is reported to the shared counter
/// (for parallel downloads). Otherwise uses a local progress bar.
fn resumable_download(
    url: &str,
    target_dir: &Path,
    shared: Option<&Arc<SharedProgress>>,
    cancel_token: CancellationToken,
) -> Result<(PathBuf, u64)> {
    let file_name = Url::parse(url)
        .ok()
        .and_then(|u| u.path_segments()?.next_back().map(|s| s.to_string()))
        .unwrap_or_else(|| "snapshot.tar".to_string());

    let final_path = target_dir.join(&file_name);
    let part_path = target_dir.join(format!("{file_name}.part"));

    let quiet = shared.is_some();

    if !quiet {
        info!(target: "reth::cli", file = %file_name, "Connecting to download server");
    }
    let client = BlockingClient::builder().timeout(Duration::from_secs(30)).build()?;

    let mut total_size: Option<u64> = None;
    let mut last_error: Option<eyre::Error> = None;

    let finalize_download = |size: u64| -> Result<(PathBuf, u64)> {
        fs::rename(&part_path, &final_path)?;
        if !quiet {
            info!(target: "reth::cli", file = %file_name, "Download complete");
        }
        Ok((final_path.clone(), size))
    };

    for attempt in 1..=MAX_DOWNLOAD_RETRIES {
        let existing_size = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);

        if let Some(total) = total_size &&
            existing_size >= total
        {
            return finalize_download(total);
        }

        if attempt > 1 {
            info!(target: "reth::cli",
                file = %file_name,
                "Retry attempt {}/{} - resuming from {} bytes",
                attempt, MAX_DOWNLOAD_RETRIES, existing_size
            );
        }

        let mut request = client.get(url);
        if existing_size > 0 {
            request = request.header(RANGE, format!("bytes={existing_size}-"));
            if !quiet && attempt == 1 {
                info!(target: "reth::cli", file = %file_name, "Resuming from {} bytes", existing_size);
            }
        }

        let response = match request.send().and_then(|r| r.error_for_status()) {
            Ok(r) => r,
            Err(e) => {
                last_error = Some(e.into());
                if attempt < MAX_DOWNLOAD_RETRIES {
                    info!(target: "reth::cli",
                        file = %file_name,
                        "Download failed, retrying in {RETRY_BACKOFF_SECS}s..."
                    );
                    std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
                }
                continue;
            }
        };

        let is_partial = response.status() == StatusCode::PARTIAL_CONTENT;

        let size = if is_partial {
            response
                .headers()
                .get("Content-Range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split('/').next_back())
                .and_then(|v| v.parse().ok())
        } else {
            response.content_length()
        };

        if total_size.is_none() {
            total_size = size;
            if !quiet && let Some(s) = size {
                info!(target: "reth::cli",
                    file = %file_name,
                    size = %DownloadProgress::format_size(s),
                    "Downloading"
                );
            }
        }

        let current_total = total_size.ok_or_else(|| {
            eyre::eyre!("Server did not provide Content-Length or Content-Range header")
        })?;

        let file = if is_partial && existing_size > 0 {
            OpenOptions::new()
                .append(true)
                .open(&part_path)
                .map_err(|e| fs::FsPathError::open(e, &part_path))?
        } else {
            fs::create_file(&part_path)?
        };

        let start_offset = if is_partial { existing_size } else { 0 };
        let mut reader = response;

        let copy_result;
        let flush_result;

        if let Some(sp) = shared {
            // Parallel path: bump shared atomic counter
            if start_offset > 0 {
                sp.add(start_offset);
            }
            let mut writer =
                SharedProgressWriter { inner: BufWriter::new(file), progress: Arc::clone(sp) };
            copy_result = io::copy(&mut reader, &mut writer);
            flush_result = writer.inner.flush();
        } else {
            // Legacy single-download path: local progress bar
            let mut progress = DownloadProgress::new(current_total);
            progress.downloaded = start_offset;
            let mut writer = ProgressWriter {
                inner: BufWriter::new(file),
                progress,
                cancel_token: cancel_token.clone(),
            };
            copy_result = io::copy(&mut reader, &mut writer);
            flush_result = writer.inner.flush();
            println!();
        }

        if let Err(e) = copy_result.and(flush_result) {
            last_error = Some(e.into());
            if attempt < MAX_DOWNLOAD_RETRIES {
                info!(target: "reth::cli",
                    file = %file_name,
                    "Download interrupted, retrying in {RETRY_BACKOFF_SECS}s..."
                );
                std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
            }
            continue;
        }

        return finalize_download(current_total);
    }

    Err(last_error
        .unwrap_or_else(|| eyre::eyre!("Download failed after {} attempts", MAX_DOWNLOAD_RETRIES)))
}

/// Streams a remote archive directly into the extractor without writing to disk.
///
/// On failure, retries from scratch up to [`MAX_DOWNLOAD_RETRIES`] times.
fn streaming_download_and_extract(
    url: &str,
    format: CompressionFormat,
    target_dir: &Path,
    shared: Option<&Arc<SharedProgress>>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let quiet = shared.is_some();
    let mut last_error: Option<eyre::Error> = None;

    for attempt in 1..=MAX_DOWNLOAD_RETRIES {
        if attempt > 1 {
            info!(target: "reth::cli",
                url = %url,
                attempt,
                max = MAX_DOWNLOAD_RETRIES,
                "Retrying streaming download from scratch"
            );
        }

        let client = BlockingClient::builder().connect_timeout(Duration::from_secs(30)).build()?;

        let response = match client.get(url).send().and_then(|r| r.error_for_status()) {
            Ok(r) => r,
            Err(e) => {
                last_error = Some(e.into());
                if attempt < MAX_DOWNLOAD_RETRIES {
                    std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
                }
                continue;
            }
        };

        if !quiet && let Some(size) = response.content_length() {
            info!(target: "reth::cli",
                url = %url,
                size = %DownloadProgress::format_size(size),
                "Streaming archive"
            );
        }

        let result = if let Some(sp) = shared {
            let reader = SharedProgressReader { inner: response, progress: Arc::clone(sp) };
            extract_archive_raw(reader, format, target_dir)
        } else {
            let total_size = response.content_length().unwrap_or(0);
            extract_archive(response, total_size, format, target_dir, cancel_token.clone())
        };

        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_error = Some(e);
                if attempt < MAX_DOWNLOAD_RETRIES {
                    std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        eyre::eyre!("Streaming download failed after {MAX_DOWNLOAD_RETRIES} attempts")
    }))
}

/// Fetches the snapshot from a remote URL with resume support, then extracts it.
fn download_and_extract(
    url: &str,
    format: CompressionFormat,
    target_dir: &Path,
    shared: Option<&Arc<SharedProgress>>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let quiet = shared.is_some();
    let (downloaded_path, total_size) =
        resumable_download(url, target_dir, shared, cancel_token.clone())?;

    let file_name =
        downloaded_path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();

    if !quiet {
        info!(target: "reth::cli",
            file = %file_name,
            size = %DownloadProgress::format_size(total_size),
            "Extracting archive"
        );
    }
    let file = fs::open(&downloaded_path)?;

    if quiet {
        // Skip progress tracking for extraction in parallel mode
        extract_archive_raw(file, format, target_dir)?;
    } else {
        extract_archive(file, total_size, format, target_dir, cancel_token)?;
        info!(target: "reth::cli",
            file = %file_name,
            "Extraction complete"
        );
    }

    fs::remove_file(&downloaded_path)?;

    if let Some(sp) = shared {
        sp.archive_done();
    }

    Ok(())
}

/// Downloads and extracts a snapshot, blocking until finished.
///
/// Supports `file://` URLs for local files and HTTP(S) URLs for remote downloads.
/// When `resumable` is true, downloads to a `.part` file first with HTTP Range resume
/// support. Otherwise streams directly into the extractor.
fn blocking_download_and_extract(
    url: &str,
    target_dir: &Path,
    shared: Option<Arc<SharedProgress>>,
    resumable: bool,
    cancel_token: CancellationToken,
) -> Result<()> {
    let format = CompressionFormat::from_url(url)?;

    if let Ok(parsed_url) = Url::parse(url) &&
        parsed_url.scheme() == "file"
    {
        let file_path = parsed_url
            .to_file_path()
            .map_err(|_| eyre::eyre!("Invalid file:// URL path: {}", url))?;
        let result = extract_from_file(&file_path, format, target_dir);
        if result.is_ok() &&
            let Some(sp) = shared
        {
            sp.archive_done();
        }
        result
    } else if resumable {
        download_and_extract(url, format, target_dir, shared.as_ref(), cancel_token)
    } else {
        let result =
            streaming_download_and_extract(url, format, target_dir, shared.as_ref(), cancel_token);
        if result.is_ok() &&
            let Some(sp) = shared
        {
            sp.archive_done();
        }
        result
    }
}

/// Downloads and extracts a snapshot archive asynchronously.
///
/// When `shared` is provided, download progress is reported to the shared
/// counter for aggregated display. Otherwise uses a local progress bar.
/// When `resumable` is true, uses two-phase download with `.part` files.
async fn stream_and_extract(
    url: &str,
    target_dir: &Path,
    shared: Option<Arc<SharedProgress>>,
    resumable: bool,
    cancel_token: CancellationToken,
) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let url = url.to_string();
    task::spawn_blocking(move || {
        blocking_download_and_extract(&url, &target_dir, shared, resumable, cancel_token)
    })
    .await??;

    Ok(())
}

async fn process_modular_archive(
    planned: PlannedArchive,
    target_dir: &Path,
    cache_dir: Option<&Path>,
    shared: Option<Arc<SharedProgress>>,
    resumable: bool,
    cancel_token: CancellationToken,
) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let cache_dir = cache_dir.map(Path::to_path_buf);

    task::spawn_blocking(move || {
        blocking_process_modular_archive(
            &planned,
            &target_dir,
            cache_dir.as_deref(),
            shared,
            resumable,
            cancel_token,
        )
    })
    .await??;

    Ok(())
}

fn blocking_process_modular_archive(
    planned: &PlannedArchive,
    target_dir: &Path,
    cache_dir: Option<&Path>,
    shared: Option<Arc<SharedProgress>>,
    resumable: bool,
    cancel_token: CancellationToken,
) -> Result<()> {
    let archive = &planned.archive;
    if verify_output_files(target_dir, &archive.output_files)? {
        if let Some(sp) = &shared {
            sp.add(archive.size);
            sp.archive_done();
        }
        info!(target: "reth::cli", file = %archive.file_name, component = %planned.component, "Skipping already verified plain files");
        return Ok(());
    }

    let format = CompressionFormat::from_url(&archive.file_name)?;
    for attempt in 1..=MAX_DOWNLOAD_RETRIES {
        cleanup_output_files(target_dir, &archive.output_files);

        if resumable {
            let cache_dir = cache_dir.ok_or_else(|| eyre::eyre!("Missing cache directory"))?;
            let archive_path = cache_dir.join(&archive.file_name);
            let part_path = cache_dir.join(format!("{}.part", archive.file_name));
            let (downloaded_path, _downloaded_size) =
                resumable_download(&archive.url, cache_dir, shared.as_ref(), cancel_token.clone())?;
            let file = fs::open(&downloaded_path)?;
            extract_archive_raw(file, format, target_dir)?;
            let _ = fs::remove_file(&archive_path);
            let _ = fs::remove_file(&part_path);
        } else {
            streaming_download_and_extract(
                &archive.url,
                format,
                target_dir,
                shared.as_ref(),
                cancel_token.clone(),
            )?;
        }

        if verify_output_files(target_dir, &archive.output_files)? {
            if let Some(sp) = &shared {
                sp.archive_done();
            }
            return Ok(());
        }

        warn!(target: "reth::cli", file = %archive.file_name, component = %planned.component, attempt, "Extracted files failed integrity checks, retrying");
    }

    eyre::bail!(
        "Failed integrity validation after {} attempts for {}",
        MAX_DOWNLOAD_RETRIES,
        archive.file_name
    )
}

fn verify_output_files(target_dir: &Path, output_files: &[OutputFileChecksum]) -> Result<bool> {
    if output_files.is_empty() {
        return Ok(false);
    }

    for expected in output_files {
        let output_path = target_dir.join(&expected.path);
        let meta = match fs::metadata(&output_path) {
            Ok(meta) => meta,
            Err(_) => return Ok(false),
        };
        if meta.len() != expected.size {
            return Ok(false);
        }

        let actual = file_blake3_hex(&output_path)?;
        if !actual.eq_ignore_ascii_case(&expected.blake3) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn cleanup_output_files(target_dir: &Path, output_files: &[OutputFileChecksum]) {
    for output in output_files {
        let _ = fs::remove_file(target_dir.join(&output.path));
    }
}

fn file_blake3_hex(path: &Path) -> Result<String> {
    let mut file = fs::open(path)?;
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

/// Discovers the latest snapshot manifest URL for the given chain from the snapshots API.
///
/// Queries `snapshots.reth.rs/api/snapshots` and returns the manifest URL for the most
/// recent modular snapshot matching the requested chain.
async fn discover_manifest_url(chain_id: u64) -> Result<String> {
    let api_url = RETH_SNAPSHOTS_API_URL;

    info!(target: "reth::cli", %api_url, %chain_id, "Discovering latest snapshot manifest");

    let entries = fetch_snapshot_api_entries(chain_id).await?;

    let entry =
        entries.iter().filter(|s| s.is_modular()).max_by_key(|s| s.block).ok_or_else(|| {
            eyre::eyre!(
                "No modular snapshot manifest found for chain \
                 {chain_id} at {api_url}\n\n\
                 You can provide a manifest URL directly with --manifest-url, or\n\
                 use a direct snapshot URL with -u from:\n\
                 \t- https://snapshots.reth.rs\n\n\
                 Use --list to see all available snapshots."
            )
        })?;

    info!(target: "reth::cli",
        block = entry.block,
        url = %entry.metadata_url,
        "Found latest snapshot manifest"
    );

    Ok(entry.metadata_url.clone())
}

/// Deserializes a JSON value that may be either a number or a string-encoded number.
fn deserialize_string_or_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = serde_json::Value::deserialize(deserializer)?;
    match &value {
        serde_json::Value::Number(n) => {
            n.as_u64().ok_or_else(|| serde::de::Error::custom("expected u64"))
        }
        serde_json::Value::String(s) => {
            s.parse::<u64>().map_err(|_| serde::de::Error::custom("expected numeric string"))
        }
        _ => Err(serde::de::Error::custom("expected number or string")),
    }
}

/// An entry from the `snapshots.reth.rs/api/snapshots` listing.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotApiEntry {
    #[serde(deserialize_with = "deserialize_string_or_u64")]
    chain_id: u64,
    #[serde(deserialize_with = "deserialize_string_or_u64")]
    block: u64,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    metadata_url: String,
    #[serde(default)]
    size: u64,
}

impl SnapshotApiEntry {
    fn is_modular(&self) -> bool {
        self.metadata_url.ends_with("manifest.json")
    }
}

/// Fetches the full snapshot listing from the snapshots API, filtered by chain ID.
async fn fetch_snapshot_api_entries(chain_id: u64) -> Result<Vec<SnapshotApiEntry>> {
    let api_url = RETH_SNAPSHOTS_API_URL;

    let entries: Vec<SnapshotApiEntry> = Client::new()
        .get(api_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .wrap_err_with(|| format!("Failed to fetch snapshot listing from {api_url}"))?
        .json()
        .await?;

    Ok(entries.into_iter().filter(|e| e.chain_id == chain_id).collect())
}

/// Prints a formatted table of available modular snapshots.
fn print_snapshot_listing(entries: &[SnapshotApiEntry], chain_id: u64) {
    let modular: Vec<_> = entries.iter().filter(|e| e.is_modular()).collect();

    println!("Available snapshots for chain {chain_id} (https://snapshots.reth.rs):\n");
    println!("{:<12}  {:>10}  {:<10}  {:>10}  MANIFEST URL", "DATE", "BLOCK", "PROFILE", "SIZE");
    println!("{}", "-".repeat(100));

    for entry in &modular {
        let date = entry.date.as_deref().unwrap_or("-");
        let profile = entry.profile.as_deref().unwrap_or("-");
        let size = if entry.size > 0 {
            DownloadProgress::format_size(entry.size)
        } else {
            "-".to_string()
        };

        println!(
            "{date:<12}  {:>10}  {profile:<10}  {size:>10}  {}",
            entry.block, entry.metadata_url
        );
    }

    if modular.is_empty() {
        println!("  (no modular snapshots found)");
    }

    println!(
        "\nTo download a specific snapshot, copy its manifest URL and run:\n  \
         reth download --manifest-url <URL>"
    );
}

async fn fetch_manifest_from_source(source: &str) -> Result<SnapshotManifest> {
    if let Ok(parsed) = Url::parse(source) {
        return match parsed.scheme() {
            "http" | "https" => {
                let response = Client::new()
                    .get(source)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .wrap_err_with(|| {
                        format!(
                            "Failed to fetch snapshot manifest from {source}\n\n\
                             The manifest endpoint may not be available for this snapshot source.\n\
                             You can use a direct snapshot URL instead:\n\n\
                             \treth download -u <snapshot-url>\n\n\
                             Available snapshot sources:\n\
                             \t- https://snapshots.reth.rs\n\
                             \t- https://publicnode.com/snapshots"
                        )
                    })?;
                Ok(response.json().await?)
            }
            "file" => {
                let path = parsed
                    .to_file_path()
                    .map_err(|_| eyre::eyre!("Invalid file:// manifest path: {source}"))?;
                let content = fs::read_to_string(path)?;
                Ok(serde_json::from_str(&content)?)
            }
            _ => Err(eyre::eyre!("Unsupported manifest URL scheme: {}", parsed.scheme())),
        };
    }

    let content = fs::read_to_string(source)?;
    Ok(serde_json::from_str(&content)?)
}

fn resolve_manifest_base_url(manifest: &SnapshotManifest, source: &str) -> Result<String> {
    if let Some(base_url) = manifest.base_url.as_deref() &&
        !base_url.is_empty()
    {
        return Ok(base_url.trim_end_matches('/').to_string());
    }

    if let Ok(mut url) = Url::parse(source) {
        if url.scheme() == "file" {
            let mut path = url
                .to_file_path()
                .map_err(|_| eyre::eyre!("Invalid file:// manifest path: {source}"))?;
            path.pop();
            let mut base = Url::from_directory_path(path)
                .map_err(|_| eyre::eyre!("Invalid manifest directory for source: {source}"))?
                .to_string();
            if base.ends_with('/') {
                base.pop();
            }
            return Ok(base);
        }

        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| eyre::eyre!("manifest_url must have a hierarchical path"))?;
            segments.pop_if_empty();
            segments.pop();
        }
        return Ok(url.as_str().trim_end_matches('/').to_string());
    }

    let path = Path::new(source);
    let manifest_dir = if path.is_absolute() {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    } else {
        let joined = std::env::current_dir()?.join(path);
        joined.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    };
    let mut base = Url::from_directory_path(&manifest_dir)
        .map_err(|_| eyre::eyre!("Invalid manifest directory: {}", manifest_dir.display()))?
        .to_string();
    if base.ends_with('/') {
        base.pop();
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{ComponentManifest, SingleArchive};
    use tempfile::tempdir;

    fn manifest_with_archive_only_components() -> SnapshotManifest {
        let mut components = BTreeMap::new();
        components.insert(
            SnapshotComponentType::TransactionSenders.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "transaction_senders.tar.zst".to_string(),
                size: 1,
                blake3: None,
                output_files: vec![],
            }),
        );
        components.insert(
            SnapshotComponentType::RocksdbIndices.key().to_string(),
            ComponentManifest::Single(SingleArchive {
                file: "rocksdb_indices.tar.zst".to_string(),
                size: 1,
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
        assert!(help.contains("snapshots.reth.rs"));
        assert!(help.contains("publicnode.com"));
        assert!(help.contains("file://"));
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
    fn summarize_download_startup_counts_reusable_and_needs_download() {
        let dir = tempdir().unwrap();
        let target_dir = dir.path();
        let ok_file = target_dir.join("ok.bin");
        std::fs::write(&ok_file, vec![1_u8; 4]).unwrap();
        let ok_hash = file_blake3_hex(&ok_file).unwrap();

        let planned = vec![
            PlannedArchive {
                ty: SnapshotComponentType::State,
                component: "State".to_string(),
                archive: ArchiveDescriptor {
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
                archive: ArchiveDescriptor {
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
                archive: ArchiveDescriptor {
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
                archive: ArchiveDescriptor {
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
                archive: ArchiveDescriptor {
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
                archive: ArchiveDescriptor {
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
}
