use crate::common::EnvironmentArgs;
use clap::Parser;
use eyre::Result;
use lz4::Decoder;
use reqwest::Client;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_fs_util as fs;
use reth_metrics::{
    metrics::{self, Counter, Gauge, Histogram},
    Metrics,
};
use reth_node_core::version::version_metadata;
use reth_node_metrics::{
    chain::ChainSpecInfo,
    hooks::Hooks,
    server::{MetricServer, MetricServerConfig},
    version::VersionInfo,
};
use std::{
    borrow::Cow,
    io::{self, Read, Write},
    net::SocketAddr,
    path::Path,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tar::Archive;
use tokio::task;
use tracing::info;
use zstd::stream::read::Decoder as ZstdDecoder;

const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const MERKLE_BASE_URL: &str = "https://downloads.merkle.io";
const EXTENSION_TAR_LZ4: &str = ".tar.lz4";
const EXTENSION_TAR_ZSTD: &str = ".tar.zst";

/// Global static download defaults
static DOWNLOAD_DEFAULTS: OnceLock<DownloadDefaults> = OnceLock::new();

/// Metrics for the download command
#[derive(Metrics, Clone)]
#[metrics(scope = "cli.download")]
struct DownloadMetrics {
    /// Successful downloads
    downloads_success_total: Counter,
    /// Failed downloads
    downloads_failed_total: Counter,
    /// Download duration
    download_duration_seconds: Histogram,
    /// Extraction duration
    extraction_duration_seconds: Histogram,
    /// Download speed (bytes/sec)
    download_speed_bytes_per_second: Gauge,
    /// Total bytes downloaded
    downloaded_bytes_total: Counter,
    /// File size
    download_file_size_bytes: Gauge,
    /// Progress (0-100)
    download_progress_percent: Gauge,
}

/// Download configuration defaults
///
/// Global defaults can be set via [`DownloadDefaults::try_init`].
#[derive(Debug, Clone)]
pub struct DownloadDefaults {
    /// List of available snapshot sources
    pub available_snapshots: Vec<Cow<'static, str>>,
    /// Default base URL for snapshots
    pub default_base_url: Cow<'static, str>,
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
            "\nIf no URL is provided, the latest mainnet archive snapshot\nwill be proposed for download from ",
        );
        help.push_str(self.default_base_url.as_ref());
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

    /// Custom URL to download the snapshot from
    #[arg(long, short, long_help = DownloadDefaults::get_global().long_help())]
    url: Option<String>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET")]
    metrics: Option<SocketAddr>,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> DownloadCommand<C> {
    pub async fn execute(self, ctx: CliContext) -> Result<()> {
        let data_dir = self.env.datadir.resolve_datadir(self.env.chain.chain());
        fs::create_dir_all(&data_dir)?;

        // Start metrics server if requested
        if let Some(listen_addr) = self.metrics {
            let config = MetricServerConfig::new(
                listen_addr,
                VersionInfo {
                    version: version_metadata().cargo_pkg_version.as_ref(),
                    build_timestamp: version_metadata().vergen_build_timestamp.as_ref(),
                    cargo_features: version_metadata().vergen_cargo_features.as_ref(),
                    git_sha: version_metadata().vergen_git_sha.as_ref(),
                    target_triple: version_metadata().vergen_cargo_target_triple.as_ref(),
                    build_profile: version_metadata().build_profile_name.as_ref(),
                },
                ChainSpecInfo { name: self.env.chain.chain().to_string() },
                ctx.task_executor,
                Hooks::builder().build(),
            );

            MetricServer::new(config).serve().await?;
            info!(target: "reth::cli", "Metrics server started at {}", listen_addr);
        }

        let url = match self.url {
            Some(url) => url,
            None => {
                let url = get_latest_snapshot_url().await?;
                info!(target: "reth::cli", "Using default snapshot URL: {}", url);
                url
            }
        };

        info!(target: "reth::cli",
            chain = %self.env.chain.chain(),
            dir = ?data_dir.data_dir(),
            url = %url,
            "Starting snapshot download and extraction"
        );

        let metrics = DownloadMetrics::default();
        let start = Instant::now();
        let result = stream_and_extract(&url, data_dir.data_dir(), &metrics).await;

        match result {
            Ok(_) => {
                let elapsed = start.elapsed();
                info!(target: "reth::cli",
                    "Snapshot downloaded and extracted successfully in {:.2}s",
                    elapsed.as_secs_f64()
                );
                Ok(())
            }
            Err(e) => {
                info!(target: "reth::cli",
                    "Snapshot download failed after {:.2}s: {}",
                    start.elapsed().as_secs_f64(), e
                );
                Err(e)
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
struct DownloadProgress<'a> {
    downloaded: u64,
    total_size: u64,
    last_displayed: Instant,
    last_speed_update: Instant,
    last_speed_bytes: u64,
    metrics: &'a DownloadMetrics,
}

impl<'a> DownloadProgress<'a> {
    fn new(total_size: u64, metrics: &'a DownloadMetrics) -> Self {
        metrics.download_file_size_bytes.set(total_size as f64);
        metrics.download_progress_percent.set(0.0);

        Self {
            downloaded: 0,
            total_size,
            last_displayed: Instant::now(),
            last_speed_update: Instant::now(),
            last_speed_bytes: 0,
            metrics,
        }
    }

    /// Converts bytes to human readable format (B, KB, MB, GB)
    fn format_size(size: u64) -> String {
        let mut size = size as f64;
        let mut unit_index = 0;

        while size >= 1024.0 && unit_index < BYTE_UNITS.len() - 1 {
            size /= 1024.0;
            unit_index += 1;
        }

        format!("{:.2} {}", size, BYTE_UNITS[unit_index])
    }

    fn update(&mut self, chunk_size: u64) -> Result<()> {
        self.downloaded += chunk_size;

        self.metrics.downloaded_bytes_total.increment(chunk_size);
        let progress = (self.downloaded as f64 / self.total_size as f64) * 100.0;
        self.metrics.download_progress_percent.set(progress);
        if self.last_speed_update.elapsed() >= Duration::from_secs(1) {
            let bytes_since_last = self.downloaded - self.last_speed_bytes;
            let elapsed = self.last_speed_update.elapsed().as_secs_f64();
            let speed = bytes_since_last as f64 / elapsed;
            self.metrics.download_speed_bytes_per_second.set(speed);
            self.last_speed_bytes = self.downloaded;
            self.last_speed_update = Instant::now();
        }

        if self.last_displayed.elapsed() >= Duration::from_millis(100) {
            let formatted_downloaded = Self::format_size(self.downloaded);
            let formatted_total = Self::format_size(self.total_size);

            print!(
                "\rDownloading and extracting... {progress:.2}% ({formatted_downloaded} / {formatted_total})",
            );
            io::stdout().flush()?;
            self.last_displayed = Instant::now();
        }

        Ok(())
    }
}

/// Adapter to track progress while reading
struct ProgressReader<'a, R> {
    reader: R,
    progress: DownloadProgress<'a>,
}

impl<'a, R: Read> ProgressReader<'a, R> {
    fn new(reader: R, total_size: u64, metrics: &'a DownloadMetrics) -> Self {
        Self { reader, progress: DownloadProgress::new(total_size, metrics) }
    }
}

impl<'a, R: Read> Read for ProgressReader<'a, R> {
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
        if url.ends_with(EXTENSION_TAR_LZ4) {
            Ok(Self::Lz4)
        } else if url.ends_with(EXTENSION_TAR_ZSTD) {
            Ok(Self::Zstd)
        } else {
            Err(eyre::eyre!("Unsupported file format. Expected .tar.lz4 or .tar.zst, got: {}", url))
        }
    }
}

/// Downloads and extracts a snapshot, blocking until finished.
fn blocking_download_and_extract(
    url: &str,
    target_dir: &Path,
    metrics: &DownloadMetrics,
) -> Result<()> {
    let overall_start = Instant::now();

    let client = reqwest::blocking::Client::builder().build()?;
    let response = client.get(url).send()?.error_for_status()?;

    let total_size = response.content_length().ok_or_else(|| {
        eyre::eyre!(
            "Server did not provide Content-Length header. This is required for snapshot downloads"
        )
    })?;

    let progress_reader = ProgressReader::new(response, total_size, metrics);
    let format = CompressionFormat::from_url(url)?;

    let extraction_start = Instant::now();
    let result = match format {
        CompressionFormat::Lz4 => {
            let decoder = Decoder::new(progress_reader)?;
            Archive::new(decoder).unpack(target_dir)?;
            Ok(())
        }
        CompressionFormat::Zstd => {
            let decoder = ZstdDecoder::new(progress_reader)?;
            Archive::new(decoder).unpack(target_dir)?;
            Ok(())
        }
    };

    metrics.extraction_duration_seconds.record(extraction_start.elapsed());
    metrics.download_duration_seconds.record(overall_start.elapsed());

    match &result {
        Ok(_) => {
            metrics.downloads_success_total.increment(1);
            metrics.download_progress_percent.set(100.0);
            metrics.download_speed_bytes_per_second.set(0.0);
            info!(target: "reth::cli", "Extraction complete.");
        }
        Err(_) => {
            metrics.downloads_failed_total.increment(1);
        }
    }

    result
}

async fn stream_and_extract(url: &str, target_dir: &Path, metrics: &DownloadMetrics) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let url = url.to_string();
    let metrics = metrics.clone();
    task::spawn_blocking(move || blocking_download_and_extract(&url, &target_dir, &metrics))
        .await??;

    Ok(())
}

// Builds default URL for latest mainnet archive snapshot using configured defaults
async fn get_latest_snapshot_url() -> Result<String> {
    let base_url = &DownloadDefaults::get_global().default_base_url;
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
}
