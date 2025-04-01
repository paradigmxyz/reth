//! CLI command to show configs.

use clap::Parser;
use eyre::{bail, WrapErr};
use reth_chainspec::ChainSpec;
use reth_config::Config;
use std::{path::PathBuf, sync::Arc};
/// `reth config` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PathBuf>,

    /// Show the default config
    #[arg(long, verbatim_doc_comment, conflicts_with = "config")]
    default: bool,
}

impl Command {
    /// Execute `config` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let config = if self.default {
            Config::default()
        } else {
            let path = self.config.clone().unwrap_or_default();
            // Check if the file exists
            if !path.exists() {
                bail!("Config file does not exist: {}", path.display());
            }
            // Read the configuration file
            Config::from_path(&path)
                .wrap_err_with(|| format!("Could not load config file: {}", path.display()))?
        };
        println!("{}", toml::to_string_pretty(&config)?);
        Ok(())
    }
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<ChainSpec>> {
        None
    }
}
