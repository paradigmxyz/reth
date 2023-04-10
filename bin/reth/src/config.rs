//! CLI command to show configs
use clap::Parser;
use eyre::{bail, WrapErr};
use reth_staged_sync::Config;

use crate::dirs::{ConfigPath, PlatformPath};

/// `reth config` command
#[derive(Debug, Parser)]
pub struct Command {
    #[clap(subcommand)]
    command: Subcommands,
}

/// `reth config` subcommands
#[derive(Debug, Parser)]
pub enum Subcommands {
    /// Show the default config
    Default,
    /// Show the active config
    Show {
        /// The path to the configuration file to use.
        #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
        config: PlatformPath<ConfigPath>,
    },
}

impl Command {
    /// Execute `config` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let config = match &self.command {
            Subcommands::Default => Config::default(),
            Subcommands::Show { config } => {
                // confy will create the file if it doesn't exist; we don't want this
                if !config.as_ref().exists() {
                    bail!("Config file does not exist: {}", config.as_ref().display());
                }
                confy::load_path::<Config>(&config).wrap_err_with(|| {
                    format!("Could not load config file: {}", config.as_ref().display())
                })?
            }
        };
        println!("{}", toml::to_string_pretty(&config)?);
        Ok(())
    }
}
