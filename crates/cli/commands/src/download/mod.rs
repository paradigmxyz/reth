pub mod config_gen;
pub mod manifest;
pub mod manifest_cmd;
mod tui;

use crate::common::EnvironmentArgs;
use clap::Parser;
use config_gen::{config_for_selections, write_config, write_prune_checkpoints};
use eyre::Result;
use futures::stream::{self, StreamExt};
use lz4::Decoder;
use manifest::{ComponentSelection, SnapshotComponentType, SnapshotManifest};
use reqwest::{blocking::Client as BlockingClient, header::RANGE, Client, StatusCode};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_db::init_db;
use reth_fs_util as fs;
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
use tracing::info;
use tui::{run_selector, SelectionResult};
use url::Url;
use zstd::stream::read::Decoder as ZstdDecoder;

const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const MERKLE_BASE_URL: &str = "https://downloads.merkle.io";
const EXTENSION_TAR_LZ4: &str = ".tar.lz4";
const EXTENSION_TAR_ZSTD: &str = ".tar.zst";

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

    /// Default download configuration with defaults from merkle.io and publicnode
    pub fn default_download_defaults() -> Self {
        Self {
            available_snapshots: vec![
                Cow::Borrowed("https://www.merkle.io/snapshots (default, mainnet archive)"),
                Cow::Borrowed("https://publicnode.com/snapshots (full nodes & testnets)"),
            ],
            default_base_url: Cow::Borrowed(MERKLE_BASE_URL),
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
            "Specify a snapshot URL or let the command propose a default one.\n\nAvailable snapshot sources:\n",
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

#[derive(Debug, Parser)]
pub struct DownloadCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Custom URL to download a single snapshot archive (legacy mode).
    ///
    /// When provided, downloads and extracts a single archive without component selection.
    #[arg(long, short, long_help = DownloadDefaults::get_global().long_help())]
    url: Option<String>,

    /// URL to a snapshot manifest.json for modular component downloads.
    ///
    /// When provided, fetches this manifest instead of discovering it from the default
    /// base URL. Useful for testing with custom or local manifests.
    #[arg(long, value_name = "URL", conflicts_with = "url")]
    manifest_url: Option<String>,

    /// Include transaction static files.
    #[arg(long)]
    with_txs: bool,

    /// Include receipt static files.
    #[arg(long)]
    with_receipts: bool,

    /// Include account and storage history static files.
    #[arg(long, alias = "with-changesets")]
    with_state_history: bool,

    /// Download all available components (full archive).
    #[arg(long, conflicts_with_all = ["with_txs", "with_receipts", "with_state_history"])]
    all: bool,

    /// Skip interactive component selection. Downloads the minimal set
    /// (state + headers + transactions + changesets) unless explicit --with-* flags narrow it.
    #[arg(long, short = 'y')]
    non_interactive: bool,

    /// Skip reth.toml generation after download.
    #[arg(long)]
    no_config: bool,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> DownloadCommand<C> {
    pub async fn execute<N>(self) -> Result<()> {
        let chain = self.env.chain.chain();
        let chain_id = chain.id();
        let data_dir = self.env.datadir.clone().resolve_datadir(chain);
        fs::create_dir_all(&data_dir)?;

        // Legacy single-URL mode: download one archive and extract it
        if let Some(url) = self.url {
            info!(target: "reth::cli",
                dir = ?data_dir.data_dir(),
                url = %url,
                "Starting snapshot download and extraction"
            );

            stream_and_extract(&url, data_dir.data_dir(), None).await?;
            info!(target: "reth::cli", "Snapshot downloaded and extracted successfully");

            return Ok(());
        }

        // Modular download: fetch manifest and select components
        let manifest_url = match &self.manifest_url {
            Some(url) => url.clone(),
            None => {
                let base_url = get_base_url(chain_id);
                format!("{base_url}/manifest.json")
            }
        };

        info!(target: "reth::cli", url = %manifest_url, "Fetching snapshot manifest");
        let manifest = manifest::fetch_manifest(&manifest_url).await?;

        info!(target: "reth::cli",
            block = manifest.block,
            chain_id = manifest.chain_id,
            storage_version = %manifest.storage_version,
            components = manifest.components.len(),
            "Loaded snapshot manifest"
        );

        let selections = self.resolve_components(&manifest)?;

        // Collect all archive URLs across all selected components
        let target_dir = data_dir.data_dir();
        let mut all_downloads: Vec<(String, String)> = Vec::new();
        for (ty, sel) in &selections {
            let distance = match sel {
                ComponentSelection::All => None,
                ComponentSelection::Distance(d) => Some(*d),
                ComponentSelection::None => continue,
            };
            let urls = manifest.archive_urls_for_distance(*ty, distance);
            let name = ty.display_name().to_string();

            if !urls.is_empty() {
                info!(target: "reth::cli",
                    component = %name,
                    archives = urls.len(),
                    selection = %sel,
                    "Queued component for download"
                );
            }

            for url in urls {
                all_downloads.push((url, name.clone()));
            }
        }

        let total_archives = all_downloads.len();
        let total_size: u64 = selections
            .iter()
            .map(|(ty, sel)| match sel {
                ComponentSelection::All => manifest.size_for_distance(*ty, None),
                ComponentSelection::Distance(d) => manifest.size_for_distance(*ty, Some(*d)),
                ComponentSelection::None => 0,
            })
            .sum();
        info!(target: "reth::cli",
            archives = total_archives,
            total = %DownloadProgress::format_size(total_size),
            "Downloading all archives"
        );

        // Download and extract all archives in parallel (up to 4 concurrent)
        let shared = SharedProgress::new(total_size, total_archives as u64);
        let progress_handle = spawn_progress_display(Arc::clone(&shared));

        let target = target_dir.to_path_buf();
        let results: Vec<Result<()>> = stream::iter(all_downloads)
            .map(|(url, _component)| {
                let dir = target.clone();
                let sp = Arc::clone(&shared);
                async move {
                    stream_and_extract(&url, &dir, Some(sp)).await?;
                    Ok(())
                }
            })
            .buffer_unordered(4)
            .collect()
            .await;

        shared.done.store(true, Ordering::Relaxed);
        let _ = progress_handle.await;

        // Check for errors
        for result in results {
            result?;
        }

        // Generate reth.toml and set prune checkpoints
        let config = config_for_selections(&selections);
        if !self.no_config && write_config(&config, target_dir)? {
            let desc = config_gen::describe_prune_config_from_selections(&selections);
            info!(target: "reth::cli", "{}", desc.join(", "));
        }

        // Write prune checkpoints to the DB so the pruner knows data before the
        // snapshot block is already in the expected pruned state
        if config.prune.segments != Default::default() {
            let db_path = data_dir.db();
            let db = init_db(&db_path, self.env.db.database_args())?;
            write_prune_checkpoints(&db, &config, manifest.block)?;
        }

        info!(target: "reth::cli", "Snapshot download complete. Run `reth node` to start syncing.");

        Ok(())
    }

    /// Determines which components to download based on CLI flags or interactive selection.
    fn resolve_components(
        &self,
        manifest: &SnapshotManifest,
    ) -> Result<BTreeMap<SnapshotComponentType, ComponentSelection>> {
        let available = |ty: SnapshotComponentType| manifest.component(ty).is_some();

        // --all: everything available as All
        if self.all {
            return Ok(SnapshotComponentType::ALL
                .iter()
                .copied()
                .filter(|ty| available(*ty))
                .map(|ty| (ty, ComponentSelection::All))
                .collect());
        }

        let has_explicit_flags = self.with_txs || self.with_receipts || self.with_state_history;

        if has_explicit_flags {
            let mut selections = BTreeMap::new();
            // Required components always All
            selections.insert(SnapshotComponentType::State, ComponentSelection::All);
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
            return Ok(selections);
        }

        if self.non_interactive {
            // Minimal defaults
            let mut selections = BTreeMap::new();
            for ty in SnapshotComponentType::ALL.iter().copied().filter(|ty| available(*ty)) {
                let sel = if ty.is_required() {
                    ComponentSelection::All
                } else if ty.is_minimal() {
                    ComponentSelection::Distance(10_064)
                } else {
                    ComponentSelection::None
                };
                // Only include components that are not None
                if sel != ComponentSelection::None {
                    selections.insert(ty, sel);
                }
            }
            return Ok(selections);
        }

        // Interactive TUI
        match run_selector(manifest.clone())? {
            SelectionResult::Selected(selections) => {
                // Filter out None selections
                Ok(selections
                    .into_iter()
                    .filter(|(_, sel)| *sel != ComponentSelection::None)
                    .collect())
            }
            SelectionResult::Cancelled => {
                eyre::bail!("Download cancelled by user");
            }
        }
    }
}

impl<C: ChainSpecParser> DownloadCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

// Monitor process status and display progress every 100ms
// to avoid overwhelming stdout
pub(crate) struct DownloadProgress {
    downloaded: u64,
    total_size: u64,
    last_displayed: Instant,
    started_at: Instant,
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
}

impl SharedProgress {
    fn new(total_size: u64, total_archives: u64) -> Arc<Self> {
        Arc::new(Self {
            downloaded: AtomicU64::new(0),
            total_size,
            total_archives,
            archives_done: AtomicU64::new(0),
            done: AtomicBool::new(false),
        })
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
}

impl<R: Read> ProgressReader<R> {
    fn new(reader: R, total_size: u64) -> Self {
        Self { reader, progress: DownloadProgress::new(total_size) }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
) -> Result<()> {
    let progress_reader = ProgressReader::new(reader, total_size);

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
fn extract_archive_raw<R: Read>(reader: R, format: CompressionFormat, target_dir: &Path) -> Result<()> {
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
    extract_archive(file, total_size, format, target_dir)?;
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
}

impl<W: Write> Write for ProgressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
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
        let n = self.inner.write(buf)?;
        self.progress.add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
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
            let mut writer = ProgressWriter { inner: BufWriter::new(file), progress };
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

/// Fetches the snapshot from a remote URL with resume support, then extracts it.
fn download_and_extract(
    url: &str,
    format: CompressionFormat,
    target_dir: &Path,
    shared: Option<&Arc<SharedProgress>>,
) -> Result<()> {
    let quiet = shared.is_some();
    let (downloaded_path, total_size) = resumable_download(url, target_dir, shared)?;

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
        extract_archive(file, total_size, format, target_dir)?;
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
/// Supports both `file://` URLs for local files and HTTP(S) URLs for remote downloads.
fn blocking_download_and_extract(
    url: &str,
    target_dir: &Path,
    shared: Option<Arc<SharedProgress>>,
) -> Result<()> {
    let format = CompressionFormat::from_url(url)?;

    if let Ok(parsed_url) = Url::parse(url) &&
        parsed_url.scheme() == "file"
    {
        let file_path = parsed_url
            .to_file_path()
            .map_err(|_| eyre::eyre!("Invalid file:// URL path: {}", url))?;
        extract_from_file(&file_path, format, target_dir)
    } else {
        download_and_extract(url, format, target_dir, shared.as_ref())
    }
}

/// Downloads and extracts a snapshot archive asynchronously.
///
/// When `shared` is provided, download progress is reported to the shared
/// counter for aggregated display. Otherwise uses a local progress bar.
async fn stream_and_extract(
    url: &str,
    target_dir: &Path,
    shared: Option<Arc<SharedProgress>>,
) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let url = url.to_string();
    task::spawn_blocking(move || blocking_download_and_extract(&url, &target_dir, shared))
        .await??;

    Ok(())
}

/// Builds the base URL for the given chain ID using configured defaults.
fn get_base_url(chain_id: u64) -> String {
    let defaults = DownloadDefaults::get_global();
    match &defaults.default_chain_aware_base_url {
        Some(url) => format!("{url}/{chain_id}"),
        None => defaults.default_base_url.to_string(),
    }
}

/// Builds default URL for latest mainnet archive snapshot using configured defaults.
///
/// Used by the legacy single-archive download flow when no manifest is available.
#[allow(dead_code)]
async fn get_latest_snapshot_url(chain_id: u64) -> Result<String> {
    let base_url = get_base_url(chain_id);
    let latest_url = format!("{base_url}/latest.txt");
    let filename = Client::new()
        .get(latest_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?
        .trim()
        .to_string();

    Ok(format!("{base_url}/{filename}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(help.contains("merkle.io"));
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
}
