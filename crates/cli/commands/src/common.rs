//! Contains common `reth` arguments

use clap::Parser;
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::ChainSpec;
use reth_config::{config::EtlConfig, Config};
use reth_db::{init_db, open_db_read_only, DatabaseEnv};
use reth_db_common::init::init_genesis;
use reth_downloaders::{bodies::noop::NoopBodiesDownloader, headers::noop::NoopHeaderDownloader};
use reth_evm::noop::NoopBlockExecutorProvider;
use reth_node_core::{
    args::{
        utils::{chain_help, chain_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs, DatadirArgs,
    },
    dirs::{ChainPath, DataDirPath},
};
use reth_primitives::B256;
use reth_provider::{providers::StaticFileProvider, ProviderFactory, StaticFileProviderFactory};
use reth_stages::{sets::DefaultStages, Pipeline, PipelineTarget};
use reth_static_file::StaticFileProducer;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Struct to hold config and datadir paths
#[derive(Debug, Parser)]
pub struct EnvironmentArgs {
    /// Parameters for datadir configuration
    #[command(flatten)]
    pub datadir: DatadirArgs,

    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = chain_value_parser
    )]
    pub chain: Arc<ChainSpec>,

    /// All database related arguments
    #[command(flatten)]
    pub db: DatabaseArgs,
}

impl EnvironmentArgs {
    /// Initializes environment according to [`AccessRights`] and returns an instance of
    /// [`Environment`].
    pub fn init(&self, access: AccessRights) -> eyre::Result<Environment> {
        let data_dir = self.datadir.clone().resolve_datadir(self.chain.chain);
        let db_path = data_dir.db();
        let sf_path = data_dir.static_files();

        if access.is_read_write() {
            reth_fs_util::create_dir_all(&db_path)?;
            reth_fs_util::create_dir_all(&sf_path)?;
        }

        let config_path = self.config.clone().unwrap_or_else(|| data_dir.config());

        let mut config = Config::from_path(config_path)
            .inspect_err(
                |err| warn!(target: "reth::cli", %err, "Failed to load config file, using default"),
            )
            .unwrap_or_default();

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

        let provider_factory = self.create_provider_factory(&config, db, sfp)?;
        if access.is_read_write() {
            debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");
            init_genesis(provider_factory.clone())?;
        }

        Ok(Environment { config, provider_factory, data_dir })
    }

    /// Returns a [`ProviderFactory`] after executing consistency checks.
    ///
    /// If it's a read-write environment and an issue is found, it will attempt to heal (including a
    /// pipeline unwind). Otherwise, it will print out an warning, advising the user to restart the
    /// node to heal.
    fn create_provider_factory(
        &self,
        config: &Config,
        db: Arc<DatabaseEnv>,
        static_file_provider: StaticFileProvider,
    ) -> eyre::Result<ProviderFactory<Arc<DatabaseEnv>>> {
        let has_receipt_pruning = config.prune.as_ref().map_or(false, |a| a.has_receipts_pruning());
        let prune_modes =
            config.prune.as_ref().map(|prune| prune.segments.clone()).unwrap_or_default();
        let factory = ProviderFactory::new(db, self.chain.clone(), static_file_provider)
            .with_prune_modes(prune_modes.clone());

        // Check for consistency between database and static files.
        if let Some(unwind_target) = factory
            .static_file_provider()
            .check_consistency(&factory.provider()?, has_receipt_pruning)?
        {
            if factory.db_ref().is_read_only() {
                warn!(target: "reth::cli", ?unwind_target, "Inconsistent storage. Restart node to heal.");
                return Ok(factory)
            }

            // Highly unlikely to happen, and given its destructive nature, it's better to panic
            // instead.
            assert_ne!(unwind_target, PipelineTarget::Unwind(0), "A static file <> database inconsistency was found that would trigger an unwind to block 0");

            info!(target: "reth::cli", unwind_target = %unwind_target, "Executing an unwind after a failed storage consistency check.");

            let (_tip_tx, tip_rx) = watch::channel(B256::ZERO);

            // Builds and executes an unwind-only pipeline
            let mut pipeline = Pipeline::builder()
                .add_stages(DefaultStages::new(
                    factory.clone(),
                    tip_rx,
                    Arc::new(EthBeaconConsensus::new(self.chain.clone())),
                    NoopHeaderDownloader::default(),
                    NoopBodiesDownloader::default(),
                    NoopBlockExecutorProvider::default(),
                    config.stages.clone(),
                    prune_modes.clone(),
                ))
                .build(factory.clone(), StaticFileProducer::new(factory.clone(), prune_modes));

            // Move all applicable data from database to static files.
            pipeline.move_to_static_files()?;
            pipeline.unwind(unwind_target.unwind_target().expect("should exist"), None)?;
        }

        Ok(factory)
    }
}

/// Environment built from [`EnvironmentArgs`].
#[derive(Debug)]
pub struct Environment {
    /// Configuration for reth node
    pub config: Config,
    /// Provider factory.
    pub provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    /// Datadir path.
    pub data_dir: ChainPath<DataDirPath>,
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
