//! Contains common `reth` arguments

use clap::Parser;
use reth_config::{config::EtlConfig, Config};
use reth_db::{init_db, open_db_read_only, DatabaseEnv};
use reth_db_common::init::init_genesis;
use reth_node_core::args::{
    utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
    DatabaseArgs, DatadirArgs,
};
use reth_primitives::ChainSpec;
use reth_provider::{providers::StaticFileProvider, ProviderFactory};
use std::{path::PathBuf, sync::Arc};
use tracing::{debug, info};

/// Struct to hold config and datadir paths
#[derive(Debug, Parser)]
pub struct EnvironmentArgs {
    #[command(flatten)]
    datadir: DatadirArgs,

    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    pub config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    pub chain: Arc<ChainSpec>,

    /// All database related arguments
    #[command(flatten)]
    pub db: DatabaseArgs,
}

impl EnvironmentArgs {
    /// Initializes environment according to [`AccessRights`] and returns a tuple of [Config] &
    /// [`ProviderFactory`].
    pub fn init(&self, access: AccessRights) -> eyre::Result<Environment> {
        let data_dir = self.datadir.clone().resolve_datadir(self.chain.chain);
        let db_path = data_dir.db();
        let sf_path = data_dir.static_files();

        if access.is_read_write() {
            reth_fs_util::create_dir_all(&db_path)?;
            reth_fs_util::create_dir_all(&sf_path)?;
        }

        let config_path = self.config.clone().unwrap_or_else(|| data_dir.config());
        let mut config: Config = confy::load_path(config_path).unwrap_or_default();

        // Make sure ETL doesn't default to /tmp/, but to whatever datadir is set to
        if config.stages.etl.dir.is_none() {
            config.stages.etl.dir = Some(EtlConfig::from_datadir(data_dir.data_dir()));
        }

        info!(target: "reth::cli", ?db_path, ?sf_path, "Opening storage");
        let (db, sfp) = match access {
            AccessRights::RW => (
                Arc::new(init_db(db_path, self.db.database_args())?),
                StaticFileProvider::read_write(sf_path)?,
            ),
            AccessRights::RO => (
                Arc::new(open_db_read_only(&db_path, self.db.database_args())?),
                StaticFileProvider::read_only(sf_path)?,
            ),
        };

        let provider_factory = ProviderFactory::new(db, self.chain.clone(), sfp);
        if access.is_read_write() {
            debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");
            init_genesis(provider_factory.clone())?;
        }

        Ok(Environment { config, provider_factory })
    }
}

/// Environment built from [`EnvironmentArgs`].
#[derive(Debug)]
pub struct Environment {
    /// Configuration for reth node
    pub config: Config,
    /// Provider factory.
    pub provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
}

/// Environment access rights.
#[derive(Debug, Copy, Clone)]
pub enum AccessRights {
    /// Read-write access
    RW,
    /// Read-only access
    RO,
}

impl AccessRights {
    /// Returns `true` if it requires read-write access to the environment.
    pub const fn is_read_write(&self) -> bool {
        matches!(self, Self::RW)
    }
}
