use clap::Parser;
use eyre::{eyre, Result};
use reth_cli_runner::CliContext;
use reth_node_ethereum::EthereumNode;
use reth_provider::providers::ReadOnlyConfig;
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory},
    Nibbles,
};
use reth_trie_db::DatabaseTrieCursorFactory;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::Instant,
};
use tracing::info;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Measures performance of TrieCursor iteration over accounts trie"
)]
pub struct TrieCursorPerfCommand {
    /// Path to datadir
    #[arg(long, value_name = "PATH")]
    pub datadir: PathBuf,

    /// Path to file containing paths to seek (overrides default iteration behavior)
    #[arg(long, value_name = "PATH")]
    pub seek: Option<PathBuf>,
}

impl TrieCursorPerfCommand {
    pub async fn execute(self, _ctx: CliContext) -> Result<()> {
        info!("Opening database at {:?}", self.datadir);

        // Open the database in read-only mode using EthereumNode
        let factory = EthereumNode::provider_factory_builder().open_read_only(
            Default::default(),
            ReadOnlyConfig::from_datadir(self.datadir.clone()),
        )?;

        // Get a provider
        let provider = factory.provider()?;
        let tx = provider.tx_ref();

        // Create the trie cursor factory
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(tx);

        // Create cursor for the accounts trie
        let mut cursor = trie_cursor_factory.account_trie_cursor()?;

        if let Some(seek_file) = self.seek {
            // Seek mode: read paths from file and seek to each one
            info!("Reading paths from {:?}", seek_file);

            let file = File::open(&seek_file)?;
            let reader = BufReader::new(file);
            let mut paths = Vec::new();

            // Read paths from file
            for line in reader.lines() {
                let line = line?;
                // Parse the line to extract the path (format: "Nibbles(0x...) <nanos>")
                if let Some(path_str) = line.split_whitespace().next() {
                    // Remove "Nibbles(" prefix and ")" suffix
                    if path_str.starts_with("Nibbles(0x") && path_str.ends_with(")") {
                        let hex_str = &path_str[10..path_str.len() - 1];
                        // Parse hex string to nibbles directly (each hex char is one nibble)
                        let mut nibble_vec = Vec::with_capacity(hex_str.len());
                        for ch in hex_str.chars() {
                            let nibble = ch.to_digit(16).ok_or_else(|| eyre!("Invalid hex character: {}", ch))? as u8;
                            nibble_vec.push(nibble);
                        }
                        let nibbles = Nibbles::from_nibbles(nibble_vec);
                        paths.push(nibbles);
                    }
                }
            }

            info!("Starting seek operations for {} paths...", paths.len());

            // Seek to each path and measure time
            for (idx, path) in paths.iter().enumerate() {
                let start = Instant::now();
                let result = cursor.seek(*path);
                let duration = start.elapsed();

                result?.ok_or(eyre!("path {path:?} not found!"))?;

                println!("{:?} {}", path, duration.as_nanos());

                if (idx + 1) % 1000 == 0 {
                    info!("Processed {} seeks", idx + 1);
                }
            }

            info!("Seek operations complete. Total paths: {}", paths.len());
        } else {
            // Default mode: iterate through trie
            info!("Starting iteration over accounts trie...");

            let mut count = 0;

            loop {
                let start = Instant::now();
                let result = cursor.next();
                let duration = start.elapsed();

                match result? {
                    Some((key, _value)) => {
                        println!("{:?} {}", key, duration.as_nanos());
                        count += 1;
                    }
                    None => {
                        info!("Iteration complete. Total nodes: {}", count);
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}
