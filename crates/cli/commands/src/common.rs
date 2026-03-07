//! Contains common `reth` arguments

pub use reth_primitives_traits::header::HeaderMut;

use alloy_primitives::B256;
use clap::Parser;
use reth_chainspec::EthChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_config::{config::EtlConfig, Config};
use reth_consensus::noop::NoopConsensus;
use reth_db::{init_db, open_db_read_only, DatabaseEnv};
use reth_db_common::init::init_genesis_with_settings;
use reth_downloaders::{bodies::noop::NoopBodiesDownloader, headers::noop::NoopHeaderDownloader};
use reth_eth_wire::NetPrimitivesFor;
use reth_evm::{noop::NoopEvmConfig, ConfigureEvm};
use reth_network::NetworkEventListenerProvider;
use reth_node_api::FullNodeTypesAdapter;
use reth_node_builder::{
    Node, NodeComponents, NodeComponentsBuilder, NodeTypes, NodeTypesWithDBAdapter,
};
use reth_node_core::{
    args::{DatabaseArgs, DatadirArgs, StaticFilesArgs, StorageArgs},
    dirs::{ChainPath, DataDirPath},
};
use reth_provider::{
    providers::{
        BlockchainProvider, NodeTypesForProvider, RocksDBProvider, StaticFileProvider,
        StaticFileProviderBuilder,
    },
    ProviderFactory, StaticFileProviderFactory, StorageSettings,
};
use reth_stages::{sets::DefaultStages, Pipeline, PipelineTarget};
use reth_static_file::StaticFileProducer;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Struct to hold config and datadir paths
#[derive(Debug, Parser)]
pub struct EnvironmentArgs<C: ChainSpecParser> {
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
        long_help = C::help_message(),
        default_value = C::default_value(),
        value_parser = C::parser(),
        global = true
    )]
    pub chain: Arc<C::ChainSpec>,

    /// All database related arguments
    #[command(flatten)]
    pub db: DatabaseArgs,

    /// All static files related arguments
    #[command(flatten)]
    pub static_files: StaticFilesArgs,

    /// Storage mode configuration (v2 vs v1/legacy)
    #[command(flatten)]
    pub storage: StorageArgs,
}

impl<C: ChainSpecParser> EnvironmentArgs<C> {
    /// Returns the effective storage settings derived from `--storage.v2`.
    ///
    /// The base storage mode is determined by `--storage.v2`:
    /// - When `--storage.v2` is set: uses [`StorageSettings::v2()`] defaults
    /// - Otherwise: uses [`StorageSettings::base()`] defaults
    pub fn storage_settings(&self) -> StorageSettings {
        if self.storage.v2 {
            StorageSettings::v2()
        } else {
            StorageSettings::base()
        }
    }

    /// Initializes environment according to [`AccessRights`] and returns an instance of
    /// [`Environment`].
    ///
    /// The provided `runtime` is used for parallel storage I/O.
    pub fn init<N: CliNodeTypes>(
        &self,
        access: AccessRights,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<Environment<N>>
    where
        C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    {
        let data_dir = self.datadir.clone().resolve_datadir(self.chain.chain());
        let db_path = data_dir.db();
        let sf_path = data_dir.static_files();
        let rocksdb_path = data_dir.rocksdb();

        reth_fs_util::create_dir_all(&db_path)?;
        reth_fs_util::create_dir_all(&sf_path)?;
        reth_fs_util::create_dir_all(&rocksdb_path)?;

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
        if config.stages.era.folder.is_none() {
            config.stages.era = config.stages.era.with_datadir(data_dir.data_dir());
        }

        info!(target: "reth::cli", ?db_path, ?sf_path, "Opening storage");
        let genesis_block_number = self.chain.genesis().number.unwrap_or_default();
        let (db, sfp) = match access {
            AccessRights::RW => (
                init_db(db_path, self.db.database_args())?,
                StaticFileProviderBuilder::read_write(sf_path)
                    .with_metrics()
                    .with_genesis_block_number(genesis_block_number)
                    .build()?,
            ),
            AccessRights::RO | AccessRights::RoInconsistent => {
                (open_db_read_only(&db_path, self.db.database_args())?, {
                    let provider = StaticFileProviderBuilder::read_only(sf_path)
                        .with_metrics()
                        .with_genesis_block_number(genesis_block_number)
                        .build()?;
                    provider.watch_directory();
                    provider
                })
            }
        };
        let rocksdb_provider = if !access.is_read_write() && !RocksDBProvider::exists(&rocksdb_path)
        {
            // RocksDB database doesn't exist yet (e.g. datadir restored from a snapshot
            // or created before RocksDB storage). Create an empty one so read-only
            // commands can proceed.
            debug!(target: "reth::cli", ?rocksdb_path, "RocksDB not found, initializing empty database");
            reth_fs_util::create_dir_all(&rocksdb_path)?;
            RocksDBProvider::builder(data_dir.rocksdb())
                .with_default_tables()
                .with_database_log_level(self.db.log_level)
                .build()?
        } else {
            RocksDBProvider::builder(data_dir.rocksdb())
                .with_default_tables()
                .with_database_log_level(self.db.log_level)
                .with_read_only(!access.is_read_write())
                .build()?
        };

        let provider_factory =
            self.create_provider_factory(&config, db, sfp, rocksdb_provider, access, runtime)?;
        if access.is_read_write() {
            debug!(target: "reth::cli", chain=%self.chain.chain(), genesis=?self.chain.genesis_hash(), "Initializing genesis");
            init_genesis_with_settings(&provider_factory, self.storage_settings())?;
        }

        Ok(Environment { config, provider_factory, data_dir })
    }

    /// Returns a [`ProviderFactory`] after executing consistency checks.
    ///
    /// If it's a read-write environment and an issue is found, it will attempt to heal (including a
    /// pipeline unwind). Otherwise, it will print out a warning, advising the user to restart the
    /// node to heal.
    fn create_provider_factory<N: CliNodeTypes>(
        &self,
        config: &Config,
        db: DatabaseEnv,
        static_file_provider: StaticFileProvider<N::Primitives>,
        rocksdb_provider: RocksDBProvider,
        access: AccessRights,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<ProviderFactory<NodeTypesWithDBAdapter<N, DatabaseEnv>>>
    where
        C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    {
        let prune_modes = config.prune.segments.clone();
        let factory = ProviderFactory::<NodeTypesWithDBAdapter<N, DatabaseEnv>>::new(
            db,
            self.chain.clone(),
            static_file_provider,
            rocksdb_provider,
            runtime,
        )?
        .with_prune_modes(prune_modes.clone());

        // Check for consistency between database and static files.
        if !access.is_read_only_inconsistent() &&
            let Some(unwind_target) =
                factory.static_file_provider().check_consistency(&factory.provider()?)?
        {
            if factory.db_ref().is_read_only()? {
                warn!(target: "reth::cli", ?unwind_target, "Inconsistent storage. Restart node to heal.");
                return Ok(factory)
            }

            // Highly unlikely to happen, and given its destructive nature, it's better to panic
            // instead.
            assert_ne!(
                unwind_target,
                PipelineTarget::Unwind(0),
                "A static file <> database inconsistency was found that would trigger an unwind to block 0"
            );

            info!(target: "reth::cli", unwind_target = %unwind_target, "Executing an unwind after a failed storage consistency check.");

            let (_tip_tx, tip_rx) = watch::channel(B256::ZERO);

            // Builds and executes an unwind-only pipeline
            let mut pipeline = Pipeline::<NodeTypesWithDBAdapter<N, DatabaseEnv>>::builder()
                .add_stages(DefaultStages::new(
                    factory.clone(),
                    tip_rx,
                    Arc::new(NoopConsensus::default()),
                    NoopHeaderDownloader::default(),
                    NoopBodiesDownloader::default(),
                    NoopEvmConfig::<N::Evm>::default(),
                    config.stages.clone(),
                    prune_modes.clone(),
                    None,
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
pub struct Environment<N: NodeTypes> {
    /// Configuration for reth node
    pub config: Config,
    /// Provider factory.
    pub provider_factory: ProviderFactory<NodeTypesWithDBAdapter<N, DatabaseEnv>>,
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
    /// Read-only access with possibly inconsistent data
    RoInconsistent,
}

impl AccessRights {
    /// Returns `true` if it requires read-write access to the environment.
    pub const fn is_read_write(&self) -> bool {
        matches!(self, Self::RW)
    }

    /// Returns `true` if it requires read-only access to the environment with possibly inconsistent
    /// data.
    pub const fn is_read_only_inconsistent(&self) -> bool {
        matches!(self, Self::RoInconsistent)
    }
}

/// Helper alias to satisfy `FullNodeTypes` bound on [`Node`] trait generic.
type FullTypesAdapter<T> = FullNodeTypesAdapter<
    T,
    DatabaseEnv,
    BlockchainProvider<NodeTypesWithDBAdapter<T, DatabaseEnv>>,
>;

/// Helper trait with a common set of requirements for the
/// [`NodeTypes`] in CLI.
pub trait CliNodeTypes: Node<FullTypesAdapter<Self>> + NodeTypesForProvider {
    type Evm: ConfigureEvm<Primitives = Self::Primitives>;
    type NetworkPrimitives: NetPrimitivesFor<Self::Primitives>;
}

impl<N> CliNodeTypes for N
where
    N: Node<FullTypesAdapter<Self>> + NodeTypesForProvider,
{
    type Evm = <<N::ComponentsBuilder as NodeComponentsBuilder<FullTypesAdapter<Self>>>::Components as NodeComponents<FullTypesAdapter<Self>>>::Evm;
    type NetworkPrimitives = <<<N::ComponentsBuilder as NodeComponentsBuilder<FullTypesAdapter<Self>>>::Components as NodeComponents<FullTypesAdapter<Self>>>::Network as NetworkEventListenerProvider>::Primitives;
}

type EvmFor<N> = <<<N as Node<FullTypesAdapter<N>>>::ComponentsBuilder as NodeComponentsBuilder<
    FullTypesAdapter<N>,
>>::Components as NodeComponents<FullTypesAdapter<N>>>::Evm;

type ConsensusFor<N> =
    <<<N as Node<FullTypesAdapter<N>>>::ComponentsBuilder as NodeComponentsBuilder<
        FullTypesAdapter<N>,
    >>::Components as NodeComponents<FullTypesAdapter<N>>>::Consensus;

/// Helper trait aggregating components required for the CLI.
pub trait CliNodeComponents<N: CliNodeTypes>: Send + Sync + 'static {
    /// Returns the configured EVM.
    fn evm_config(&self) -> &EvmFor<N>;
    /// Returns the consensus implementation.
    fn consensus(&self) -> &ConsensusFor<N>;
}

impl<N: CliNodeTypes> CliNodeComponents<N> for (EvmFor<N>, ConsensusFor<N>) {
    fn evm_config(&self) -> &EvmFor<N> {
        &self.0
    }

    fn consensus(&self) -> &ConsensusFor<N> {
        &self.1
    }
}

/// Helper trait alias for an [`FnOnce`] producing [`CliNodeComponents`].
pub trait CliComponentsBuilder<N: CliNodeTypes>:
    FnOnce(Arc<N::ChainSpec>) -> Self::Components + Send + Sync + 'static
{
    type Components: CliNodeComponents<N>;
}

impl<N: CliNodeTypes, F, Comp> CliComponentsBuilder<N> for F
where
    F: FnOnce(Arc<N::ChainSpec>) -> Comp + Send + Sync + 'static,
    Comp: CliNodeComponents<N>,
{
    type Components = Comp;
}
