use clap::Parser;
use eyre::Result;
use reqwest::Client;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::args::DatadirArgs;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Child, Command as ProcessCommand, Stdio},
    sync::Arc,
    time::Instant,
};
use tracing::info;

// 1MB chunks
const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
const MERKLE_BASE_URL: &str = "https://downloads.merkle.io/";

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

        // URL handling logic
        let url = if let Some(url) = self.url {
            url
        } else {
            let latest_url = get_latest_snapshot_url().await?;
            info!(target: "reth::cli", "No URL specified. Latest snapshot available as mainnet archive: {}", latest_url);

            print!("Do you want to use this snapshot? [Y/n] ");
            std::io::stdout().flush()?;

            let mut response = String::new();
            std::io::stdin().read_line(&mut response)?;

            match response.trim().to_lowercase().as_str() {
                "" | "y" | "yes" => latest_url,
                _ => return Err(eyre::eyre!("Please specify a snapshot URL using --url")),
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

/// Spawns lz4 process for streaming decompression
fn spawn_lz4_process() -> Result<Child> {
    ProcessCommand::new("lz4")
        .arg("-d") // Decompress
        .arg("-") // Read from stdin
        .arg("-") // Write to stdout
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                eyre::eyre!("lz4 command not found. Please ensure lz4 is installed on your system")
            }
            _ => e.into(),
        })
}

/// Spawns tar process to extract streaming data to target directory
fn spawn_tar_process(target_dir: &Path, lz4_stdout: Stdio) -> Result<Child> {
    Ok(ProcessCommand::new("tar")
        .arg("-xf")
        .arg("-") // Read from stdin
        .arg("-C")
        .arg(target_dir)
        .stdin(lz4_stdout)
        .stderr(Stdio::inherit())
        .spawn()?)
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

    /// Updates progress bar and ensures child processes are still running
    fn update(&mut self, chunk_size: u64, lz4: &mut Child, tar: &mut Child) -> Result<()> {
        self.downloaded += chunk_size;

        if self.last_displayed.elapsed() >= std::time::Duration::from_millis(100) {
            // Check process status
            if let Ok(Some(status)) = lz4.try_wait() {
                return Err(eyre::eyre!("lz4 process exited prematurely with status: {}", status));
            }
            if let Ok(Some(status)) = tar.try_wait() {
                return Err(eyre::eyre!("tar process exited prematurely with status: {}", status));
            }
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

    // Require content-length for progress tracking and download validation
    let total_size = response.content_length().ok_or_else(|| {
        eyre::eyre!(
            "Server did not provide Content-Length header. This is required for snapshot downloads"
        )
    })?;

    let mut global_progress: DownloadProgress = DownloadProgress::new(total_size);

    // Setup processing pipeline: download -> lz4 -> tar
    let mut lz4_process = spawn_lz4_process()?;
    let mut tar_process = spawn_tar_process(
        target_dir,
        lz4_process.stdout.take().expect("Failed to get lz4 stdout").into(),
    )?;

    let mut lz4_stdin = lz4_process.stdin.take().expect("Failed to get lz4 stdin");

    // Stream download chunks through the pipeline
    while let Some(chunk) = response.chunk().await? {
        lz4_stdin.write_all(&chunk)?;
        global_progress.update(chunk.len() as u64, &mut lz4_process, &mut tar_process)?;
    }

    // Cleanup and verify process completion
    drop(lz4_stdin);
    let lz4_status = lz4_process.wait()?;
    let tar_status = tar_process.wait()?;

    if !lz4_status.success() {
        return Err(eyre::eyre!(
            "lz4 process failed with exit code: {}",
            lz4_status.code().unwrap_or(-1)
        ));
    }

    if !tar_status.success() {
        return Err(eyre::eyre!(
            "tar process failed with exit code: {}",
            tar_status.code().unwrap_or(-1)
        ));
    }

    Ok(())
}

// Builds default URL for latest r mainnet archive  snapshot
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

    if !filename.ends_with(".tar.lz4") {
        return Err(eyre::eyre!("Unexpected snapshot filename format: {}", filename));
    }

    Ok(format!("{}/{}", MERKLE_BASE_URL, filename))
}
