use crate::common::EnvironmentArgs;
use clap::Parser;
use eyre::Result;
use lz4::Decoder;
use reqwest::Client;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_fs_util as fs;
use std::{
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tar::Archive;
use tokio::task;
use tracing::info;

const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const MERKLE_BASE_URL: &str = "https://downloads.merkle.io";
const EXTENSION_TAR_FILE: &str = ".tar.lz4";

#[derive(Debug, Parser)]
pub struct DownloadCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[arg(
        long,
        short,
        help = "Custom URL to download the snapshot from",
        long_help = "Specify a snapshot URL or let the command propose a default one.\n\
        \n\
        Available snapshot sources:\n\
        - https://downloads.merkle.io (default, mainnet archive)\n\
        - https://publicnode.com/snapshots (full nodes & testnets)\n\
        \n\
        If no URL is provided, the latest mainnet archive snapshot\n\
        will be proposed for download from merkle.io"
    )]
    url: Option<String>,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> DownloadCommand<C> {
    pub async fn execute<N>(self) -> Result<()> {
        let data_dir = self.env.datadir.resolve_datadir(self.env.chain.chain());
        fs::create_dir_all(&data_dir)?;

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

        stream_and_extract(&url, data_dir.data_dir()).await?;
        info!(target: "reth::cli", "Snapshot downloaded and extracted successfully");

        Ok(())
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
struct DownloadProgress {
    downloaded: u64,
    total_size: u64,
    last_displayed: Instant,
}

impl DownloadProgress {
    /// Creates new progress tracker with given total size
    fn new(total_size: u64) -> Self {
        Self { downloaded: 0, total_size, last_displayed: Instant::now() }
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

    /// Updates progress bar
    fn update(&mut self, chunk_size: u64) -> Result<()> {
        self.downloaded += chunk_size;

        // Only update display at most 10 times per second for efficiency
        if self.last_displayed.elapsed() >= Duration::from_millis(100) {
            let formatted_downloaded = Self::format_size(self.downloaded);
            let formatted_total = Self::format_size(self.total_size);
            let progress = (self.downloaded as f64 / self.total_size as f64) * 100.0;

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
        if bytes > 0 {
            if let Err(e) = self.progress.update(bytes as u64) {
                return Err(io::Error::other(e));
            }
        }
        Ok(bytes)
    }
}

/// Downloads and extracts a snapshot with blocking approach
fn blocking_download_and_extract(url: &str, target_dir: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder().build()?;
    let response = client.get(url).send()?.error_for_status()?;

    let total_size = response.content_length().ok_or_else(|| {
        eyre::eyre!(
            "Server did not provide Content-Length header. This is required for snapshot downloads"
        )
    })?;

    let progress_reader = ProgressReader::new(response, total_size);

    let decoder = Decoder::new(progress_reader)?;
    let mut archive = Archive::new(decoder);

    archive.unpack(target_dir)?;

    info!(target: "reth::cli", "Extraction complete.");
    Ok(())
}

async fn stream_and_extract(url: &str, target_dir: &Path) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let url = url.to_string();
    task::spawn_blocking(move || blocking_download_and_extract(&url, &target_dir)).await??;

    Ok(())
}

// Builds default URL for latest mainnet archive  snapshot
async fn get_latest_snapshot_url() -> Result<String> {
    let latest_url = format!("{MERKLE_BASE_URL}/latest.txt");
    let filename = Client::new()
        .get(latest_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?
        .trim()
        .to_string();

    if !filename.ends_with(EXTENSION_TAR_FILE) {
        return Err(eyre::eyre!("Unexpected snapshot filename format: {}", filename));
    }

    Ok(format!("{MERKLE_BASE_URL}/{filename}"))
}
