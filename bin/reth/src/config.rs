//! CLI command to write default config to stdout
use clap::Parser;
use reth_staged_sync::Config;

/// `reth config` command
#[derive(Debug, Parser)]
pub struct Command;

impl Command {
    /// Execute `config` command - write default config to stdout
    pub async fn execute(&self) -> eyre::Result<()> {
        let config = Config::default();
        println!("{}", toml::to_string_pretty(&config)?);
        Ok(())
    }
}
