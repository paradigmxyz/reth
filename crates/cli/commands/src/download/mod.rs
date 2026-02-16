pub mod config_gen;
pub mod manifest;
mod tui;

use crate::common::EnvironmentArgs;
use clap::Parser;
use config_gen::{config_for_components, write_config};
use eyre::Result;
use lz4::Decoder;
use manifest::{ComponentUrlOverrides, SnapshotComponentType, SnapshotManifest};
use reqwest::{blocking::Client as BlockingClient, header::RANGE, Client, StatusCode};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_fs_util as fs;
use std::{
    borrow::Cow,
    fs::OpenOptions,
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
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

    /// Include transaction static files.
    #[arg(long)]
    with_txs: bool,

    /// Include receipt static files.
    #[arg(long)]
    with_receipts: bool,

    /// Include account and storage changeset static files.
    #[arg(long)]
    with_changesets: bool,

    /// Download all available components (full archive).
    #[arg(long, conflicts_with_all = ["with_txs", "with_receipts", "with_changesets"])]
    all: bool,

    /// Skip interactive component selection. Downloads state only unless
    /// explicit --with-* flags are provided.
    #[arg(long, short = 'y')]
    non_interactive: bool,

    /// Skip reth.toml generation after download.
    #[arg(long)]
    no_config: bool,

    /// Override URL for the state component archive.
    #[arg(long, value_name = "URL")]
    state_url: Option<String>,

    /// Override URL for the headers component archive.
    #[arg(long, value_name = "URL")]
    headers_url: Option<String>,

    /// Override URL for the transactions component archive.
    #[arg(long, value_name = "URL")]
    txs_url: Option<String>,

    /// Override URL for the receipts component archive.
    #[arg(long, value_name = "URL")]
    receipts_url: Option<String>,

    /// Override URL for the account changesets component archive.
    #[arg(long, value_name = "URL")]
    account_changesets_url: Option<String>,

    /// Override URL for the storage changesets component archive.
    #[arg(long, value_name = "URL")]
    storage_changesets_url: Option<String>,
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

            stream_and_extract(&url, data_dir.data_dir()).await?;
            info!(target: "reth::cli", "Snapshot downloaded and extracted successfully");

            return Ok(());
        }

        // Modular download: discover available components by convention
        let base_url = get_base_url(chain_id);
        let overrides = ComponentUrlOverrides {
            state: self.state_url.clone(),
            headers: self.headers_url.clone(),
            transactions: self.txs_url.clone(),
            receipts: self.receipts_url.clone(),
            account_changesets: self.account_changesets_url.clone(),
            storage_changesets: self.storage_changesets_url.clone(),
        };

        info!(target: "reth::cli", "Discovering available snapshot components from {base_url}");
        let manifest = manifest::discover_components(&base_url, &overrides).await?;

        let selected = self.resolve_components(&manifest)?;

        // Download each selected component
        let target_dir = data_dir.data_dir();
        for ty in &selected {
            if let Some(component) = manifest.component(*ty) {
                info!(target: "reth::cli",
                    component = ty.display_name(),
                    size = %DownloadProgress::format_size(component.size),
                    "Downloading component"
                );
                stream_and_extract(&component.url, target_dir).await?;
                info!(target: "reth::cli",
                    component = ty.display_name(),
                    "Component downloaded and extracted"
                );
            }
        }

        // Generate reth.toml
        if !self.no_config {
            let config = config_for_components(&selected);
            if write_config(&config, target_dir)? {
                let desc = config_gen::describe_prune_config(&selected);
                for line in &desc {
                    info!(target: "reth::cli", "{}", line);
                }
            }
        }

        info!(target: "reth::cli", "Snapshot download complete. Run `reth node` to start syncing.");

        Ok(())
    }

    /// Determines which components to download based on CLI flags or interactive selection.
    fn resolve_components(
        &self,
        manifest: &SnapshotManifest,
    ) -> Result<Vec<SnapshotComponentType>> {
        // --all: download everything available in the manifest
        if self.all {
            return Ok(SnapshotComponentType::ALL
                .iter()
                .copied()
                .filter(|ty| manifest.component(*ty).is_some())
                .collect());
        }

        // Explicit --with-* flags: state + specified components
        let has_explicit_flags = self.with_txs || self.with_receipts || self.with_changesets;

        if has_explicit_flags || self.non_interactive {
            let mut selected = vec![SnapshotComponentType::State];

            if manifest.component(SnapshotComponentType::Headers).is_some() {
                selected.push(SnapshotComponentType::Headers);
            }

            if self.with_txs {
                if manifest.component(SnapshotComponentType::Transactions).is_some() {
                    selected.push(SnapshotComponentType::Transactions);
                }
            }
            if self.with_receipts {
                if manifest.component(SnapshotComponentType::Receipts).is_some() {
                    selected.push(SnapshotComponentType::Receipts);
                }
            }
            if self.with_changesets {
                if let Some(_) = manifest.component(SnapshotComponentType::AccountChangesets) {
                    selected.push(SnapshotComponentType::AccountChangesets);
                }
                if let Some(_) = manifest.component(SnapshotComponentType::StorageChangesets) {
                    selected.push(SnapshotComponentType::StorageChangesets);
                }
            }

            return Ok(selected);
        }

        // Interactive TUI selection
        match run_selector(manifest.clone())? {
            SelectionResult::Selected(selected) => Ok(selected),
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

    /// Updates progress bar
    fn update(&mut self, chunk_size: u64) -> Result<()> {
        self.downloaded += chunk_size;

        // Only update display at most 10 times per second for efficiency
        if self.last_displayed.elapsed() >= Duration::from_millis(100) {
            let formatted_downloaded = Self::format_size(self.downloaded);
            let formatted_total = Self::format_size(self.total_size);
            let progress = (self.downloaded as f64 / self.total_size as f64) * 100.0;

            // Calculate ETA based on current speed
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

            // Pad with spaces to clear any previous longer line
            print!(
                "\rDownloading and extracting... {progress:.2}% ({formatted_downloaded} / {formatted_total}) ETA: {eta_str}     ",
            );
            io::stdout().flush()?;
            self.last_displayed = Instant::now();
        }

        Ok(())
    }
}

/// Adapter to track progress while reading
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

    info!(target: "reth::cli", "Extraction complete.");
    Ok(())
}

/// Extracts a snapshot from a local file.
fn extract_from_file(path: &Path, format: CompressionFormat, target_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let total_size = file.metadata()?.len();
    extract_archive(file, total_size, format, target_dir)
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

/// Downloads a file with resume support using HTTP Range requests.
/// Automatically retries on failure, resuming from where it left off.
/// Returns the path to the downloaded file and its total size.
fn resumable_download(url: &str, target_dir: &Path) -> Result<(PathBuf, u64)> {
    let file_name = Url::parse(url)
        .ok()
        .and_then(|u| u.path_segments()?.next_back().map(|s| s.to_string()))
        .unwrap_or_else(|| "snapshot.tar".to_string());

    let final_path = target_dir.join(&file_name);
    let part_path = target_dir.join(format!("{file_name}.part"));

    let client = BlockingClient::builder().timeout(Duration::from_secs(30)).build()?;

    let mut total_size: Option<u64> = None;
    let mut last_error: Option<eyre::Error> = None;

    let finalize_download = |size: u64| -> Result<(PathBuf, u64)> {
        fs::rename(&part_path, &final_path)?;
        info!(target: "reth::cli", "Download complete: {}", final_path.display());
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
                "Retry attempt {}/{} - resuming from {} bytes",
                attempt, MAX_DOWNLOAD_RETRIES, existing_size
            );
        }

        let mut request = client.get(url);
        if existing_size > 0 {
            request = request.header(RANGE, format!("bytes={existing_size}-"));
            if attempt == 1 {
                info!(target: "reth::cli", "Resuming download from {} bytes", existing_size);
            }
        }

        let response = match request.send().and_then(|r| r.error_for_status()) {
            Ok(r) => r,
            Err(e) => {
                last_error = Some(e.into());
                if attempt < MAX_DOWNLOAD_RETRIES {
                    info!(target: "reth::cli",
                        "Download failed, retrying in {} seconds...", RETRY_BACKOFF_SECS
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
        let mut progress = DownloadProgress::new(current_total);
        progress.downloaded = start_offset;

        let mut writer = ProgressWriter { inner: BufWriter::new(file), progress };
        let mut reader = response;

        let copy_result = io::copy(&mut reader, &mut writer);
        let flush_result = writer.inner.flush();
        println!();

        if let Err(e) = copy_result.and(flush_result) {
            last_error = Some(e.into());
            if attempt < MAX_DOWNLOAD_RETRIES {
                info!(target: "reth::cli",
                    "Download interrupted, retrying in {} seconds...", RETRY_BACKOFF_SECS
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
fn download_and_extract(url: &str, format: CompressionFormat, target_dir: &Path) -> Result<()> {
    let (downloaded_path, total_size) = resumable_download(url, target_dir)?;

    info!(target: "reth::cli", "Extracting snapshot...");
    let file = fs::open(&downloaded_path)?;
    extract_archive(file, total_size, format, target_dir)?;

    fs::remove_file(&downloaded_path)?;
    info!(target: "reth::cli", "Removed downloaded archive");

    Ok(())
}

/// Downloads and extracts a snapshot, blocking until finished.
///
/// Supports both `file://` URLs for local files and HTTP(S) URLs for remote downloads.
fn blocking_download_and_extract(url: &str, target_dir: &Path) -> Result<()> {
    let format = CompressionFormat::from_url(url)?;

    if let Ok(parsed_url) = Url::parse(url) &&
        parsed_url.scheme() == "file"
    {
        let file_path = parsed_url
            .to_file_path()
            .map_err(|_| eyre::eyre!("Invalid file:// URL path: {}", url))?;
        extract_from_file(&file_path, format, target_dir)
    } else {
        download_and_extract(url, format, target_dir)
    }
}

async fn stream_and_extract(url: &str, target_dir: &Path) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let url = url.to_string();
    task::spawn_blocking(move || blocking_download_and_extract(&url, &target_dir)).await??;

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
