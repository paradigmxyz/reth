//! CLI command to show configs
use clap::Parser;
use eyre::{bail, WrapErr};
use reth_staged_sync::Config;

use crate::dirs::{ConfigPath, PlatformPath};

/// `reth config` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PlatformPath<ConfigPath>>,

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
            // confy will create the file if it doesn't exist; we don't want this
            if !path.as_ref().exists() {
                bail!("Config file does not exist: {}", path.as_ref().display());
            }
            confy::load_path::<Config>(path.as_ref()).wrap_err_with(|| {
                format!("Could not load config file: {}", path.as_ref().display())
            })?
        };
        println!("{}", toml::to_string_pretty(&config)?);
        Ok(())
    }
}
