use clap::Parser;
use eyre::Result;
use lz4::Decoder;
use reqwest::Client;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::args::DatadirArgs;
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::Path,
    sync::Arc,
    time::Instant,
};
use tar::Archive;
use tracing::info;

const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const MERKLE_BASE_URL: &str = "https://downloads.merkle.io";
const EXTENSION_TAR_FILE: &str = ".tar.lz4";

#[derive(Debug, Parser, Clone)]
pub struct Command<C: ChainSpecParser> {
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = C::help_message(),
        default_value = C::SUPPORTED_CHAINS[0],
        value_parser = C::parser()
    )]
    chain: Arc<C::ChainSpec>,

    #[command(flatten)]
    datadir: DatadirArgs,

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

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    pub async fn execute<N>(self) -> Result<()> {
        let data_dir = self.datadir.resolve_datadir(self.chain.chain());
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
            chain = %self.chain.chain(),
            dir = ?data_dir.data_dir(),
            url = %url,
            "Starting snapshot download and extraction"
        );

        stream_and_extract(&url, data_dir.data_dir()).await?;
        info!(target: "reth::cli", "Snapshot downloaded and extracted successfully");

        Ok(())
    }
}

// Monitor process status and display progress every 100ms to avoid overwhelming stdout
struct DownloadProgress {
    downloaded: u64,
    total_size: u64,
    last_displayed: std::time::Instant,
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

        format!("{:.2}{}", size, BYTE_UNITS[unit_index])
    }

    /// Updates progress bar
    fn update(&mut self, chunk_size: u64) -> Result<()> {
        self.downloaded += chunk_size;

        if self.last_displayed.elapsed() >= std::time::Duration::from_millis(100) {
            // Display progress
            let formatted_downloaded = Self::format_size(self.downloaded);
            let formatted_total = Self::format_size(self.total_size);
            let progress = (self.downloaded as f64 / self.total_size as f64) * 100.0;

            print!(
                "\rDownloading and extracting... {:.2}% ({} / {})",
                progress, formatted_downloaded, formatted_total
            );
            std::io::stdout().flush()?;

            self.last_displayed = Instant::now();
        }

        Ok(())
    }
}

/// Downloads snapshot and pipes it through lz4 decompression into tar extraction
async fn stream_and_extract(url: &str, target_dir: &Path) -> Result<()> {
    let client = Client::new();
    let mut response = client.get(url).send().await?.error_for_status()?;

    let total_size = response.content_length().ok_or_else(|| {
        eyre::eyre!(
            "Server did not provide Content-Length header. This is required for snapshot downloads"
        )
    })?;

    let mut global_progress: DownloadProgress = DownloadProgress::new(total_size);

    // Buffer to store downloaded chunks before processing
    let mut buffer = Vec::new();
    // Stream download chunks
    while let Some(chunk) = response.chunk().await? {
        buffer.extend_from_slice(&chunk);
        global_progress.update(chunk.len() as u64)?;
    }

    println!("\nDownload complete. Decompressing and extracting...");

    let cursor = Cursor::new(buffer);
    let mut decoder = Decoder::new(cursor)?;

    let mut decompressed_data = Vec::new();
    decoder.read_to_end(&mut decompressed_data)?;

    println!("Decompression complete. Extracting files...");

    let cursor = Cursor::new(decompressed_data);
    let mut archive = Archive::new(cursor);

    archive.unpack(target_dir)?;

    println!("\nExtraction complete.");

    Ok(())
}

// Builds default URL for latest mainnet archive  snapshot
async fn get_latest_snapshot_url() -> Result<String> {
    let latest_url = format!("{}/latest.txt", MERKLE_BASE_URL);
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

    Ok(format!("{}/{}", MERKLE_BASE_URL, filename))
}
