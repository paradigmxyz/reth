use clap::Parser;
use eyre::Result;
use reth_cli_runner::CliContext;
use reth_node_ethereum::EthereumNode;
use reth_provider::providers::ReadOnlyConfig;
use reth_trie::trie_cursor::{TrieCursor, TrieCursorFactory};
use reth_trie_db::DatabaseTrieCursorFactory;
use std::path::PathBuf;
use std::time::Instant;
use tracing::info;

#[derive(Debug, Parser)]
#[command(author, version, about = "Measures performance of TrieCursor iteration over accounts trie")]
pub struct TrieCursorPerfCommand {
    /// Path to datadir
    #[arg(long, value_name = "PATH")]
    pub datadir: PathBuf,
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

        info!("Starting iteration over accounts trie...");

        let mut count = 0;

        loop {
            let start = Instant::now();
            let result = cursor.next()?;
            let duration = start.elapsed();

            match result {
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

        Ok(())
    }
}
