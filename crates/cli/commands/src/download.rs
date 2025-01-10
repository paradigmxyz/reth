use clap::Parser;
use eyre::Result;
use reqwest::Client;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::args::DatadirArgs;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
};
use tracing::{info, warn};

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

    #[arg(long, short, required = true)]
    url: String,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    pub async fn execute<N>(self) -> Result<()> {
        let data_dir = self.datadir.resolve_datadir(self.chain.chain());
        fs::create_dir_all(&data_dir)?;

        info!(
            chain = %self.chain.chain(),
            dir = ?data_dir.data_dir(),
            url = %self.url,
            "Starting snapshot download and extraction"
        );

        stream_and_extract(&self.url, data_dir.data_dir()).await?;
        info!("Snapshot downloaded and extracted successfully");

        Ok(())
    }
}

async fn stream_and_extract(url: &str, target_dir: &Path) -> Result<()> {
    let client = Client::new();
    let mut response = client.get(url).send().await?.error_for_status()?;
    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded = 0u64;

    // Create lz4 decompression process
    let mut lz4_process = ProcessCommand::new("lz4")
        .arg("-d")  // Decompress
        .arg("-")   // Read from stdin
        .arg("-")   // Write to stdout
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Create tar extraction process
    let mut tar_process = ProcessCommand::new("tar")
        .arg("-xf")
        .arg("-")   // Read from stdin
        .arg("-C")
        .arg(target_dir)
        .stdin(lz4_process.stdout.take().expect("Failed to get lz4 stdout"))
        .stderr(Stdio::piped())
        .spawn()?;

    let mut lz4_stdin = lz4_process.stdin.take().expect("Failed to get lz4 stdin");
    let lz4_stderr = lz4_process.stderr.take().expect("Failed to get lz4 stderr");
    let tar_stderr = tar_process.stderr.take().expect("Failed to get tar stderr");

    // Spawn threads to monitor stderr of both processes
    let lz4_stderr_thread = std::thread::spawn(move || {
        let mut reader = io::BufReader::new(lz4_stderr);
        let mut line = String::new();
        while io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) > 0 {
            warn!("lz4 stderr: {}", line.trim());
            line.clear();
        }
    });

    let tar_stderr_thread = std::thread::spawn(move || {
        let mut reader = io::BufReader::new(tar_stderr);
        let mut line = String::new();
        while io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) > 0 {
            warn!("tar stderr: {}", line.trim());
            line.clear();
        }
    });

    let chunk_size = 1024 * 1024; // 1MB chunks
    let mut buffer = Vec::with_capacity(chunk_size);

    // Stream download chunks through the pipeline
    while let Some(chunk) = response.chunk().await? {
        buffer.extend_from_slice(&chunk);
        
        while buffer.len() >= chunk_size {
            let write_size = chunk_size.min(buffer.len());
            match lz4_stdin.write_all(&buffer[..write_size]) {
                Ok(_) => {
                    buffer.drain(..write_size);
                }
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    // Check if processes are still running
                    if let Ok(Some(status)) = lz4_process.try_wait() {
                        return Err(eyre::eyre!(
                            "lz4 process exited prematurely with status: {}",
                            status
                        ));
                    }
                    if let Ok(Some(status)) = tar_process.try_wait() {
                        return Err(eyre::eyre!(
                            "tar process exited prematurely with status: {}",
                            status
                        ));
                    }
                    return Err(eyre::eyre!("Pipeline broken"));
                }
                Err(e) => return Err(e.into()),
            }
        }

        downloaded += chunk.len() as u64;
        if total_size > 0 {
            let progress = (downloaded as f64 / total_size as f64) * 100.0;
            print!("\rDownloading and extracting... {:.1}%", progress);
            std::io::stdout().flush()?;
        }
    }

    // Write any remaining data
    if !buffer.is_empty() {
        lz4_stdin.write_all(&buffer)?;
    }

    // Close stdin and wait for processes to finish
    drop(lz4_stdin);
    
    let lz4_status = lz4_process.wait()?;
    let tar_status = tar_process.wait()?;

    // Join stderr monitoring threads
    lz4_stderr_thread.join().unwrap_or(());
    tar_stderr_thread.join().unwrap_or(());

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

    println!(); // New line after progress
    Ok(())
}