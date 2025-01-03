use std::{io::Write, path::Path, sync::Arc};
use tokio::{fs, io::AsyncWriteExt};

use clap::Parser;
use eyre::Result;
use reqwest::Client;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::args::DatadirArgs;

const SNAPSHOT_FILE: &str = "snapshot.tar.lz4";

/// `reth download` command
#[derive(Debug, Parser, Clone)]
pub struct Command<C: ChainSpecParser> {
    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = C::help_message(),
        default_value = C::SUPPORTED_CHAINS[0],
        value_parser = C::parser()
    )]
    chain: Arc<C::ChainSpec>,

    /// Path where will be store the snapshot
    #[command(flatten)]
    datadir: DatadirArgs,

    /// Custom URL to download the snapshot from
    /// TODO: check if we can add public snapshots urls by default
    #[arg(long, short, required = true)]
    url: String,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute the download command
    pub async fn execute<N>(self) -> Result<()> {
        let data_dir = self.datadir.resolve_datadir(self.chain.chain());
        let snapshot_path = data_dir.data_dir().join(SNAPSHOT_FILE);
        fs::create_dir_all(&data_dir).await?;

        println!("Starting snapshot download for chain: {:?}", self.chain);
        println!("Target directory: {:?}", data_dir);
        println!("Source URL: {}", self.url);

        download_snapshot(&self.url, &snapshot_path).await?;

        println!("Snapshot downloaded successfully to {:?}", snapshot_path);
        //TODO: add decompression step
        println!(
            "Please extract the snapshot using: tar --use-compress-program=lz4 -xf {:?}",
            snapshot_path
        );

        Ok(())
    }
}

async fn download_snapshot(url: &str, target_path: &Path) -> Result<()> {
    let client = Client::new();
    let mut response = client.get(url).send().await?.error_for_status()?;

    let total_size = response.content_length().unwrap_or(0);
    let mut file = fs::File::create(&target_path).await?;
    let mut downloaded = 0u64;

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if total_size > 0 {
            let progress = (downloaded as f64 / total_size as f64) * 100.0;
            print!("\rDownloading... {:.1}%", progress);
            std::io::stdout().flush()?;
        }
    }
    println!("\nDownload complete!");

    Ok(())
}
