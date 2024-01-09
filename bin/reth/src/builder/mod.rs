//! Support for customizing the node
use super::cli::{components::RethRpcServerHandles, ext::DefaultRethNodeCommandConfig};
use crate::{
    args::{
        get_secret_key, DatabaseArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs,
        PruningArgs, RpcServerArgs, TxPoolArgs,
    },
    cli::{
        components::RethNodeComponentsImpl,
        config::{RethRpcConfig, RethTransactionPoolConfig},
        db_type::{DatabaseBuilder, DatabaseInstance},
        ext::{RethCliExt, RethNodeCommandConfig},
    },
    commands::node::{cl_events::ConsensusLayerHealthEvents, events},
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
    init::init_genesis,
    prometheus_exporter,
    utils::{get_single_header, write_peers_to_file},
    version::SHORT_VERSION,
};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{future::Either, stream, stream_select, StreamExt};
use metrics_exporter_prometheus::PrometheusHandle;
use once_cell::sync::Lazy;
use reth_auto_seal_consensus::{AutoSealBuilder, AutoSealConsensus, MiningMode};
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook},
    BeaconConsensus, BeaconConsensusEngine, BeaconConsensusEngineError,
    MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree, ShareableBlockchainTree,
};
use reth_config::{
    config::{PruneConfig, StageConfig},
    Config,
};
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    blockchain_tree::BlockchainTreeEngine,
    consensus::Consensus,
    p2p::{
        bodies::{client::BodiesClient, downloader::BodyDownloader},
        either::EitherDownloader,
        headers::{client::HeadersClient, downloader::HeaderDownloader},
    },
    RethResult,
};
use reth_network::{NetworkBuilder, NetworkConfig, NetworkEvents, NetworkHandle, NetworkManager};
use reth_network_api::{NetworkInfo, PeersInfo};
use reth_primitives::{
    constants::eip4844::{LoadKzgSettingsError, MAINNET_KZG_TRUSTED_SETUP},
    kzg::KzgSettings,
    stage::StageId,
    BlockHashOrNumber, BlockNumber, ChainSpec, DisplayHardforks, Head, SealedHeader, TxHash, B256,
    MAINNET,
};
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, BlockReader,
    BlockchainTreePendingStateProvider, CanonStateSubscriptions, HeaderProvider, HeaderSyncMode,
    ProviderFactory, StageCheckpointReader,
};
use reth_prune::PrunerBuilder;
use reth_revm::EvmProcessorFactory;
use reth_revm_inspectors::stack::Hook;
use reth_rpc_engine_api::EngineApi;
use reth_stages::{
    prelude::*,
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, IndexAccountHistoryStage,
        IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TotalDifficultyStage, TransactionLookupStage,
    },
    MetricEvent,
};
use reth_tasks::{TaskExecutor, TaskManager};
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, EthTransactionPool, TransactionPool,
    TransactionValidationTaskExecutor,
};
use secp256k1::SecretKey;
use std::{
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    runtime::Handle,
    sync::{
        mpsc::{unbounded_channel, Receiver, UnboundedSender},
        oneshot, watch,
    },
};
use tracing::*;

/// The default prometheus recorder handle. We use a global static to ensure that it is only
/// installed once.
pub static PROMETHEUS_RECORDER_HANDLE: Lazy<PrometheusHandle> =
    Lazy::new(|| prometheus_exporter::install_recorder().unwrap());

/// This includes all necessary configuration to launch the node.
/// The individual configuration options can be overwritten before launching the node.
///
/// # Example
/// ```rust
/// # use reth_tasks::{TaskManager, TaskSpawner};
/// # use reth::{
/// #     builder::NodeConfig,
/// #     cli::{
/// #         ext::DefaultRethNodeCommandConfig,
/// #     },
/// #     args::RpcServerArgs,
/// # };
/// # use reth_rpc_builder::RpcModuleSelection;
/// # use tokio::runtime::Handle;
///
/// async fn t() {
///     let handle = Handle::current();
///     let manager = TaskManager::new(handle);
///     let executor = manager.executor();
///
///     // create the builder
///     let builder = NodeConfig::default();
///
///     // configure the rpc apis
///     let mut rpc = RpcServerArgs::default().with_http().with_ws();
///     rpc.http_api = Some(RpcModuleSelection::All);
///     let builder = builder.with_rpc(rpc);
///
///     let ext = DefaultRethNodeCommandConfig::default();
///     let handle = builder.launch::<()>(ext, executor).await.unwrap();
/// }
/// ```
///
/// This can also be used to launch a node with a temporary test database. This can be done with
/// the [NodeConfig::test] method.
///
/// # Example
/// ```rust
/// # use reth_tasks::{TaskManager, TaskSpawner};
/// # use reth::{
/// #     builder::NodeConfig,
/// #     cli::{
/// #         ext::DefaultRethNodeCommandConfig,
/// #     },
/// #     args::RpcServerArgs,
/// # };
/// # use reth_rpc_builder::RpcModuleSelection;
/// # use tokio::runtime::Handle;
///
/// async fn t() {
///     let handle = Handle::current();
///     let manager = TaskManager::new(handle);
///     let executor = manager.executor();
///
///     // create the builder with a test database, using the `test` method
///     let builder = NodeConfig::test();
///
///     // configure the rpc apis
///     let mut rpc = RpcServerArgs::default().with_http().with_ws();
///     rpc.http_api = Some(RpcModuleSelection::All);
///     let builder = builder.with_rpc(rpc);
///
///     let ext = DefaultRethNodeCommandConfig::default();
///     let handle = builder.launch::<()>(ext, executor).await.unwrap();
/// }
/// ```
#[derive(Debug)]
pub struct NodeConfig {
    /// The test database
    pub database: DatabaseBuilder,

    /// The path to the configuration file to use.
    pub config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    pub chain: Arc<ChainSpec>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    pub metrics: Option<SocketAddr>,

    /// Add a new instance of a node.
    ///
    /// Configures the ports of the node to avoid conflicts with the defaults.
    /// This is useful for running multiple nodes on the same machine.
    ///
    /// Max number of instances is 200. It is chosen in a way so that it's not possible to have
    /// port numbers that conflict with each other.
    ///
    /// Changes to the following port numbers:
    /// - DISCOVERY_PORT: default + `instance` - 1
    /// - AUTH_PORT: default + `instance` * 100 - 100
    /// - HTTP_RPC_PORT: default - `instance` + 1
    /// - WS_RPC_PORT: default + `instance` * 2 - 2
    pub instance: u16,

    /// Overrides the KZG trusted setup by reading from the supplied file.
    pub trusted_setup_file: Option<PathBuf>,

    /// All networking related arguments
    pub network: NetworkArgs,

    /// All rpc related arguments
    pub rpc: RpcServerArgs,

    /// All txpool related arguments with --txpool prefix
    pub txpool: TxPoolArgs,

    /// All payload builder related arguments
    pub builder: PayloadBuilderArgs,

    /// All debug related arguments with --debug prefix
    pub debug: DebugArgs,

    /// All database related arguments
    pub db: DatabaseArgs,

    /// All dev related arguments with --dev prefix
    pub dev: DevArgs,

    /// All pruning related arguments
    pub pruning: PruningArgs,

    /// Rollup related arguments
    #[cfg(feature = "optimism")]
    pub rollup: crate::args::RollupArgs,
}

impl NodeConfig {
    /// Creates a testing [NodeConfig], causing the database to be launched ephemerally.
    pub fn test() -> Self {
        Self {
            database: DatabaseBuilder::test(),
            config: None,
            chain: MAINNET.clone(),
            metrics: None,
            instance: 1,
            trusted_setup_file: None,
            network: NetworkArgs::default(),
            rpc: RpcServerArgs::default(),
            txpool: TxPoolArgs::default(),
            builder: PayloadBuilderArgs::default(),
            debug: DebugArgs::default(),
            db: DatabaseArgs::default(),
            dev: DevArgs::default(),
            pruning: PruningArgs::default(),
            #[cfg(feature = "optimism")]
            rollup: crate::args::RollupArgs::default(),
        }
    }

    /// Set the datadir for the node
    pub fn with_datadir(mut self, datadir: MaybePlatformPath<DataDirPath>) -> Self {
        self.database = DatabaseBuilder::Real(datadir);
        self
    }

    /// Set the config file for the node
    pub fn with_config(mut self, config: impl Into<PathBuf>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the chain for the node
    pub fn with_chain(mut self, chain: Arc<ChainSpec>) -> Self {
        self.chain = chain;
        self
    }

    /// Set the metrics address for the node
    pub fn with_metrics(mut self, metrics: SocketAddr) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the instance for the node
    pub fn with_instance(mut self, instance: u16) -> Self {
        self.instance = instance;
        self
    }

    /// Set the [ChainSpec] for the node
    pub fn with_chain_spec(mut self, chain: Arc<ChainSpec>) -> Self {
        self.chain = chain;
        self
    }

    /// Set the trusted setup file for the node
    pub fn with_trusted_setup_file(mut self, trusted_setup_file: impl Into<PathBuf>) -> Self {
        self.trusted_setup_file = Some(trusted_setup_file.into());
        self
    }

    /// Set the network args for the node
    pub fn with_network(mut self, network: NetworkArgs) -> Self {
        self.network = network;
        self
    }

    /// Set the rpc args for the node
    pub fn with_rpc(mut self, rpc: RpcServerArgs) -> Self {
        self.rpc = rpc;
        self
    }

    /// Set the txpool args for the node
    pub fn with_txpool(mut self, txpool: TxPoolArgs) -> Self {
        self.txpool = txpool;
        self
    }

    /// Set the builder args for the node
    pub fn with_payload_builder(mut self, builder: PayloadBuilderArgs) -> Self {
        self.builder = builder;
        self
    }

    /// Set the debug args for the node
    pub fn with_debug(mut self, debug: DebugArgs) -> Self {
        self.debug = debug;
        self
    }

    /// Set the database args for the node
    pub fn with_db(mut self, db: DatabaseArgs) -> Self {
        self.db = db;
        self
    }

    /// Set the dev args for the node
    pub fn with_dev(mut self, dev: DevArgs) -> Self {
        self.dev = dev;
        self
    }

    /// Set the pruning args for the node
    pub fn with_pruning(mut self, pruning: PruningArgs) -> Self {
        self.pruning = pruning;
        self
    }

    /// Set the node instance number
    pub fn with_instance_number(mut self, instance: u16) -> Self {
        self.instance = instance;
        self
    }

    /// Set the rollup args for the node
    #[cfg(feature = "optimism")]
    pub fn with_rollup(mut self, rollup: crate::args::RollupArgs) -> Self {
        self.rollup = rollup;
        self
    }

    /// Launches the node, also adding any RPC extensions passed.
    ///
    /// # Example
    /// ```rust
    /// # use reth_tasks::{TaskManager, TaskSpawner};
    /// # use reth::builder::NodeConfig;
    /// # use reth::cli::{
    /// #     ext::DefaultRethNodeCommandConfig,
    /// # };
    /// # use tokio::runtime::Handle;
    ///
    /// async fn t() {
    ///     let handle = Handle::current();
    ///     let manager = TaskManager::new(handle);
    ///     let executor = manager.executor();
    ///     let builder = NodeConfig::default();
    ///     let ext = DefaultRethNodeCommandConfig::default();
    ///     let handle = builder.launch::<()>(ext, executor).await.unwrap();
    /// }
    /// ```
    pub async fn launch<E: RethCliExt>(
        mut self,
        ext: E::Node,
        executor: TaskExecutor,
    ) -> eyre::Result<NodeHandle> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        let database = std::mem::take(&mut self.database);
        let db_instance = database.init_db(self.db.log_level, self.chain.chain)?;

        match db_instance {
            DatabaseInstance::Real { db, data_dir } => {
                let builder = NodeBuilderWithDatabase { config: self, db, data_dir };
                builder.launch::<E>(ext, executor).await
            }
            DatabaseInstance::Test { db, data_dir } => {
                let builder = NodeBuilderWithDatabase { config: self, db, data_dir };
                builder.launch::<E>(ext, executor).await
            }
        }
    }

    /// Get the network secret from the given data dir
    pub fn network_secret(&self, data_dir: &ChainPath<DataDirPath>) -> eyre::Result<SecretKey> {
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret_path());
        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;
        Ok(secret_key)
    }

    /// Returns the initial pipeline target, based on whether or not the node is running in
    /// `debug.tip` mode, `debug.continuous` mode, or neither.
    ///
    /// If running in `debug.tip` mode, the configured tip is returned.
    /// Otherwise, if running in `debug.continuous` mode, the genesis hash is returned.
    /// Otherwise, `None` is returned. This is what the node will do by default.
    pub fn initial_pipeline_target(&self, genesis_hash: B256) -> Option<B256> {
        if let Some(tip) = self.debug.tip {
            // Set the provided tip as the initial pipeline target.
            debug!(target: "reth::cli", %tip, "Tip manually set");
            Some(tip)
        } else if self.debug.continuous {
            // Set genesis as the initial pipeline target.
            // This will allow the downloader to start
            debug!(target: "reth::cli", "Continuous sync mode enabled");
            Some(genesis_hash)
        } else {
            None
        }
    }

    /// Returns the max block that the node should run to, looking it up from the network if
    /// necessary
    async fn max_block<Provider, Client>(
        &self,
        network_client: &Client,
        provider: Provider,
    ) -> eyre::Result<Option<BlockNumber>>
    where
        Provider: HeaderProvider,
        Client: HeadersClient,
    {
        let max_block = if let Some(block) = self.debug.max_block {
            Some(block)
        } else if let Some(tip) = self.debug.tip {
            Some(self.lookup_or_fetch_tip(provider, network_client, tip).await?)
        } else {
            None
        };

        Ok(max_block)
    }

    /// Get the [MiningMode] from the given dev args
    pub fn mining_mode(&self, pending_transactions_listener: Receiver<TxHash>) -> MiningMode {
        if let Some(interval) = self.dev.block_time {
            MiningMode::interval(interval)
        } else if let Some(max_transactions) = self.dev.block_max_transactions {
            MiningMode::instant(max_transactions, pending_transactions_listener)
        } else {
            info!(target: "reth::cli", "No mining mode specified, defaulting to ReadyTransaction");
            MiningMode::instant(1, pending_transactions_listener)
        }
    }

    /// Build a network and spawn it
    pub async fn build_network<DB>(
        &self,
        config: &Config,
        provider_factory: ProviderFactory<DB>,
        executor: TaskExecutor,
        head: Head,
        data_dir: &ChainPath<DataDirPath>,
    ) -> eyre::Result<(ProviderFactory<DB>, NetworkBuilder<ProviderFactory<DB>, (), ()>)>
    where
        DB: Database + Unpin + Clone + 'static,
    {
        info!(target: "reth::cli", "Connecting to P2P network");
        let secret_key = self.network_secret(data_dir)?;
        let default_peers_path = data_dir.known_peers_path();
        let network_config = self.load_network_config(
            config,
            provider_factory,
            executor.clone(),
            head,
            secret_key,
            default_peers_path.clone(),
        );

        let client = network_config.client.clone();
        let builder = NetworkManager::builder(network_config).await?;
        Ok((client, builder))
    }

    /// Build the blockchain tree
    pub fn build_blockchain_tree<DB>(
        &self,
        provider_factory: ProviderFactory<DB>,
        consensus: Arc<dyn Consensus>,
        prune_config: Option<PruneConfig>,
        sync_metrics_tx: UnboundedSender<MetricEvent>,
        tree_config: BlockchainTreeConfig,
    ) -> eyre::Result<BlockchainTree<DB, EvmProcessorFactory>>
    where
        DB: Database + Unpin + Clone + 'static,
    {
        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            provider_factory.clone(),
            consensus.clone(),
            EvmProcessorFactory::new(self.chain.clone()),
        );
        let tree = BlockchainTree::new(
            tree_externals,
            tree_config,
            prune_config.clone().map(|config| config.segments),
        )?
        .with_sync_metrics_tx(sync_metrics_tx.clone());

        Ok(tree)
    }

    /// Build a transaction pool and spawn the transaction pool maintenance task
    pub fn build_and_spawn_txpool<DB, Tree>(
        &self,
        blockchain_db: &BlockchainProvider<DB, Tree>,
        head: Head,
        executor: &TaskExecutor,
        data_dir: &ChainPath<DataDirPath>,
    ) -> eyre::Result<EthTransactionPool<BlockchainProvider<DB, Tree>, InMemoryBlobStore>>
    where
        DB: Database + Unpin + Clone + 'static,
        Tree: BlockchainTreeEngine
            + BlockchainTreePendingStateProvider
            + CanonStateSubscriptions
            + Clone
            + 'static,
    {
        let blob_store = InMemoryBlobStore::default();
        let validator = TransactionValidationTaskExecutor::eth_builder(Arc::clone(&self.chain))
            .with_head_timestamp(head.timestamp)
            .kzg_settings(self.kzg_settings()?)
            .with_additional_tasks(1)
            .build_with_tasks(blockchain_db.clone(), executor.clone(), blob_store.clone());

        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, self.txpool.pool_config());
        info!(target: "reth::cli", "Transaction pool initialized");
        let transactions_path = data_dir.txpool_transactions_path();

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = blockchain_db.canonical_state_stream();
            let client = blockchain_db.clone();
            let transactions_backup_config =
                reth_transaction_pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(transactions_path);

            executor.spawn_critical_with_graceful_shutdown_signal(
                "local transactions backup task",
                |shutdown| {
                    reth_transaction_pool::maintain::backup_local_transactions_task(
                        shutdown,
                        pool.clone(),
                        transactions_backup_config,
                    )
                },
            );

            // spawn the maintenance task
            executor.spawn_critical(
                "txpool maintenance task",
                reth_transaction_pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    executor.clone(),
                    Default::default(),
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        Ok(transaction_pool)
    }

    /// Returns the [Consensus] instance to use.
    ///
    /// By default this will be a [BeaconConsensus] instance, but if the `--dev` flag is set, it
    /// will be an [AutoSealConsensus] instance.
    pub fn consensus(&self) -> Arc<dyn Consensus> {
        if self.dev.dev {
            Arc::new(AutoSealConsensus::new(Arc::clone(&self.chain)))
        } else {
            Arc::new(BeaconConsensus::new(Arc::clone(&self.chain)))
        }
    }

    /// Constructs a [Pipeline] that's wired to the network
    #[allow(clippy::too_many_arguments)]
    async fn build_networked_pipeline<DB, Client>(
        &self,
        config: &StageConfig,
        client: Client,
        consensus: Arc<dyn Consensus>,
        provider_factory: ProviderFactory<DB>,
        task_executor: &TaskExecutor,
        metrics_tx: reth_stages::MetricEventsSender,
        prune_config: Option<PruneConfig>,
        max_block: Option<BlockNumber>,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Unpin + Clone + 'static,
        Client: HeadersClient + BodiesClient + Clone + 'static,
    {
        // building network downloaders using the fetch client
        let header_downloader = ReverseHeadersDownloaderBuilder::new(config.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::new(config.bodies)
            .build(client, Arc::clone(&consensus), provider_factory.clone())
            .into_task_with(task_executor);

        let pipeline = self
            .build_pipeline(
                provider_factory,
                config,
                header_downloader,
                body_downloader,
                consensus,
                max_block,
                self.debug.continuous,
                metrics_tx,
                prune_config,
            )
            .await?;

        Ok(pipeline)
    }

    /// Loads the trusted setup params from a given file path or falls back to
    /// `MAINNET_KZG_TRUSTED_SETUP`.
    fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        if let Some(ref trusted_setup_file) = self.trusted_setup_file {
            let trusted_setup = KzgSettings::load_trusted_setup_file(trusted_setup_file)
                .map_err(LoadKzgSettingsError::KzgError)?;
            Ok(Arc::new(trusted_setup))
        } else {
            Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
        }
    }

    fn install_prometheus_recorder(&self) -> eyre::Result<PrometheusHandle> {
        Ok(PROMETHEUS_RECORDER_HANDLE.clone())
    }

    async fn start_metrics_endpoint<Metrics>(
        &self,
        prometheus_handle: PrometheusHandle,
        db: Metrics,
    ) -> eyre::Result<()>
    where
        Metrics: DatabaseMetrics + 'static + Send + Sync,
    {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::serve(
                listen_addr,
                prometheus_handle,
                db,
                metrics_process::Collector::default(),
            )
            .await?;
        }

        Ok(())
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    fn start_network<C, Pool>(
        &self,
        builder: NetworkBuilder<C, (), ()>,
        task_executor: &TaskExecutor,
        pool: Pool,
        client: C,
        data_dir: &ChainPath<DataDirPath>,
    ) -> NetworkHandle
    where
        C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
    {
        let (handle, network, txpool, eth) =
            builder.transactions(pool).request_handler(client).split_with_handle();

        task_executor.spawn_critical("p2p txpool", txpool);
        task_executor.spawn_critical("p2p eth request handler", eth);

        let default_peers_path = data_dir.known_peers_path();
        let known_peers_file = self.network.persistent_peers_file(default_peers_path);
        task_executor.spawn_critical_with_graceful_shutdown_signal(
            "p2p network task",
            |shutdown| {
                network.run_until_graceful_shutdown(shutdown, |network| {
                    write_peers_to_file(network, known_peers_file)
                })
            },
        );

        handle
    }

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    fn lookup_head<DB: Database>(&self, factory: ProviderFactory<DB>) -> RethResult<Head> {
        let provider = factory.provider()?;

        let head = provider.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default().block_number;

        let header = provider
            .header_by_number(head)?
            .expect("the header for the latest block is missing, database is corrupt");

        let total_difficulty = provider
            .header_td_by_number(head)?
            .expect("the total difficulty for the latest block is missing, database is corrupt");

        let hash = provider
            .block_hash(head)?
            .expect("the hash for the latest block is missing, database is corrupt");

        Ok(Head {
            number: head,
            hash,
            difficulty: header.difficulty,
            total_difficulty,
            timestamp: header.timestamp,
        })
    }

    /// Attempt to look up the block number for the tip hash in the database.
    /// If it doesn't exist, download the header and return the block number.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn lookup_or_fetch_tip<Provider, Client>(
        &self,
        provider: Provider,
        client: Client,
        tip: B256,
    ) -> RethResult<u64>
    where
        Provider: HeaderProvider,
        Client: HeadersClient,
    {
        let header = provider.header_by_hash_or_number(tip.into())?;

        // try to look up the header in the database
        if let Some(header) = header {
            info!(target: "reth::cli", ?tip, "Successfully looked up tip block in the database");
            return Ok(header.number)
        }

        Ok(self.fetch_tip_from_network(client, tip.into()).await?.number)
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn fetch_tip_from_network<Client>(
        &self,
        client: Client,
        tip: BlockHashOrNumber,
    ) -> RethResult<SealedHeader>
    where
        Client: HeadersClient,
    {
        info!(target: "reth::cli", ?tip, "Fetching tip block from the network.");
        loop {
            match get_single_header(&client, tip).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?tip, "Successfully fetched tip");
                    return Ok(tip_header)
                }
                Err(error) => {
                    error!(target: "reth::cli", %error, "Failed to fetch the tip. Retrying...");
                }
            }
        }
    }

    fn load_network_config<DB: Database>(
        &self,
        config: &Config,
        provider_factory: ProviderFactory<DB>,
        executor: TaskExecutor,
        head: Head,
        secret_key: SecretKey,
        default_peers_path: PathBuf,
    ) -> NetworkConfig<ProviderFactory<DB>> {
        let cfg_builder = self
            .network
            .network_config(config, self.chain.clone(), secret_key, default_peers_path)
            .with_task_executor(Box::new(executor))
            .set_head(head)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(
                self.network.addr,
                // set discovery port based on instance number
                self.network.port + self.instance - 1,
            )))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(
                self.network.addr,
                // set discovery port based on instance number
                self.network.port + self.instance - 1,
            )));

        // When `sequencer_endpoint` is configured, the node will forward all transactions to a
        // Sequencer node for execution and inclusion on L1, and disable its own txpool
        // gossip to prevent other parties in the network from learning about them.
        #[cfg(feature = "optimism")]
        let cfg_builder = cfg_builder
            .sequencer_endpoint(self.rollup.sequencer_http.clone())
            .disable_tx_gossip(self.rollup.disable_txpool_gossip);

        cfg_builder.build(provider_factory)
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_pipeline<DB, H, B>(
        &self,
        provider_factory: ProviderFactory<DB>,
        stage_config: &StageConfig,
        header_downloader: H,
        body_downloader: B,
        consensus: Arc<dyn Consensus>,
        max_block: Option<u64>,
        continuous: bool,
        metrics_tx: reth_stages::MetricEventsSender,
        prune_config: Option<PruneConfig>,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Clone + 'static,
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
    {
        let mut builder = Pipeline::builder();

        if let Some(max_block) = max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        use reth_revm_inspectors::stack::InspectorStackConfig;
        let factory = reth_revm::EvmProcessorFactory::new(self.chain.clone());

        let stack_config = InspectorStackConfig {
            use_printer_tracer: self.debug.print_inspector,
            hook: if let Some(hook_block) = self.debug.hook_block {
                Hook::Block(hook_block)
            } else if let Some(tx) = self.debug.hook_transaction {
                Hook::Transaction(tx)
            } else if self.debug.hook_all {
                Hook::All
            } else {
                Hook::None
            },
        };

        let factory = factory.with_stack_config(stack_config);

        let prune_modes = prune_config.map(|prune| prune.segments).unwrap_or_default();

        let header_mode =
            if continuous { HeaderSyncMode::Continuous } else { HeaderSyncMode::Tip(tip_rx) };
        let pipeline = builder
            .with_tip_sender(tip_tx)
            .with_metrics_tx(metrics_tx.clone())
            .add_stages(
                DefaultStages::new(
                    provider_factory.clone(),
                    header_mode,
                    Arc::clone(&consensus),
                    header_downloader,
                    body_downloader,
                    factory.clone(),
                )
                .set(
                    TotalDifficultyStage::new(consensus)
                        .with_commit_threshold(stage_config.total_difficulty.commit_threshold),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: stage_config.sender_recovery.commit_threshold,
                })
                .set(
                    ExecutionStage::new(
                        factory,
                        ExecutionStageThresholds {
                            max_blocks: stage_config.execution.max_blocks,
                            max_changes: stage_config.execution.max_changes,
                            max_cumulative_gas: stage_config.execution.max_cumulative_gas,
                        },
                        stage_config
                            .merkle
                            .clean_threshold
                            .max(stage_config.account_hashing.clean_threshold)
                            .max(stage_config.storage_hashing.clean_threshold),
                        prune_modes.clone(),
                    )
                    .with_metrics_tx(metrics_tx),
                )
                .set(AccountHashingStage::new(
                    stage_config.account_hashing.clean_threshold,
                    stage_config.account_hashing.commit_threshold,
                ))
                .set(StorageHashingStage::new(
                    stage_config.storage_hashing.clean_threshold,
                    stage_config.storage_hashing.commit_threshold,
                ))
                .set(MerkleStage::new_execution(stage_config.merkle.clean_threshold))
                .set(TransactionLookupStage::new(
                    stage_config.transaction_lookup.commit_threshold,
                    prune_modes.transaction_lookup,
                ))
                .set(IndexAccountHistoryStage::new(
                    stage_config.index_account_history.commit_threshold,
                    prune_modes.account_history,
                ))
                .set(IndexStorageHistoryStage::new(
                    stage_config.index_storage_history.commit_threshold,
                    prune_modes.storage_history,
                )),
            )
            .build(provider_factory);

        Ok(pipeline)
    }

    /// Change rpc port numbers based on the instance number, using the inner
    /// [RpcServerArgs::adjust_instance_ports] method.
    fn adjust_instance_ports(&mut self) {
        self.rpc.adjust_instance_ports(self.instance);
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            database: DatabaseBuilder::default(),
            config: None,
            chain: MAINNET.clone(),
            metrics: None,
            instance: 1,
            trusted_setup_file: None,
            network: NetworkArgs::default(),
            rpc: RpcServerArgs::default(),
            txpool: TxPoolArgs::default(),
            builder: PayloadBuilderArgs::default(),
            debug: DebugArgs::default(),
            db: DatabaseArgs::default(),
            dev: DevArgs::default(),
            pruning: PruningArgs::default(),
            #[cfg(feature = "optimism")]
            rollup: crate::args::RollupArgs::default(),
        }
    }
}

/// A version of the [NodeConfig] that has an installed database. This is used to construct the
/// [NodeHandle].
///
/// This also contains a path to a data dir that cannot be changed.
pub struct NodeBuilderWithDatabase<DB> {
    /// The node config
    pub config: NodeConfig,
    /// The database
    pub db: Arc<DB>,
    /// The data dir
    pub data_dir: ChainPath<DataDirPath>,
}

impl<DB: Database + DatabaseMetrics + DatabaseMetadata + 'static> NodeBuilderWithDatabase<DB> {
    /// Launch the node with the given extensions and executor
    pub async fn launch<E: RethCliExt>(
        mut self,
        mut ext: E::Node,
        executor: TaskExecutor,
    ) -> eyre::Result<NodeHandle> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit()?;

        // get config
        let config = self.load_config()?;

        let prometheus_handle = self.config.install_prometheus_recorder()?;
        info!(target: "reth::cli", "Database opened");

        let mut provider_factory =
            ProviderFactory::new(Arc::clone(&self.db), Arc::clone(&self.config.chain));

        // configure snapshotter
        let snapshotter = reth_snapshot::Snapshotter::new(
            provider_factory.clone(),
            self.data_dir.snapshots_path(),
            self.config.chain.snapshot_block_interval,
        )?;

        provider_factory = provider_factory.with_snapshots(
            self.data_dir.snapshots_path(),
            snapshotter.highest_snapshot_receiver(),
        )?;

        self.config.start_metrics_endpoint(prometheus_handle, Arc::clone(&self.db)).await?;

        debug!(target: "reth::cli", chain=%self.config.chain.chain, genesis=?self.config.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(Arc::clone(&self.db), self.config.chain.clone())?;

        info!(target: "reth::cli", "{}", DisplayHardforks::new(self.config.chain.hardforks()));

        let consensus = self.config.consensus();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        let prune_config = self
            .config
            .pruning
            .prune_config(Arc::clone(&self.config.chain))?
            .or(config.prune.clone());

        // configure blockchain tree
        let tree_config = BlockchainTreeConfig::default();
        let tree = self.config.build_blockchain_tree(
            provider_factory.clone(),
            consensus.clone(),
            prune_config.clone(),
            sync_metrics_tx.clone(),
            tree_config,
        )?;
        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = ShareableBlockchainTree::new(tree);
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self
            .config
            .lookup_head(provider_factory.clone())
            .wrap_err("the head block is missing")?;

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        // build transaction pool
        let transaction_pool =
            self.config.build_and_spawn_txpool(&blockchain_db, head, &executor, &self.data_dir)?;

        // build network
        let (network_client, mut network_builder) = self
            .config
            .build_network(
                &config,
                provider_factory.clone(),
                executor.clone(),
                head,
                &self.data_dir,
            )
            .await?;

        let components = RethNodeComponentsImpl {
            provider: blockchain_db.clone(),
            pool: transaction_pool.clone(),
            network: network_builder.handle(),
            task_executor: executor.clone(),
            events: blockchain_db.clone(),
        };

        // allow network modifications
        ext.configure_network(network_builder.network_mut(), &components)?;

        // launch network
        let network = self.config.start_network(
            network_builder,
            &executor,
            transaction_pool.clone(),
            network_client,
            &self.data_dir,
        );

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), enode = %network.local_node_record(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        ext.on_components_initialized(&components)?;

        debug!(target: "reth::cli", "Spawning payload builder service");
        let payload_builder =
            ext.spawn_payload_builder_service(&self.config.builder, &components)?;

        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();
        let max_block = self.config.max_block(&network_client, provider_factory.clone()).await?;

        // Configure the pipeline
        let (mut pipeline, client) = if self.config.dev.dev {
            info!(target: "reth::cli", "Starting Reth in dev mode");
            let mining_mode =
                self.config.mining_mode(transaction_pool.pending_transactions_listener());

            let (_, client, mut task) = AutoSealBuilder::new(
                Arc::clone(&self.config.chain),
                blockchain_db.clone(),
                transaction_pool.clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
                mining_mode,
            )
            .build();

            let mut pipeline = self
                .config
                .build_networked_pipeline(
                    &config.stages,
                    client.clone(),
                    Arc::clone(&consensus),
                    provider_factory.clone(),
                    &executor,
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                )
                .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            executor.spawn(Box::pin(task));

            (pipeline, EitherDownloader::Left(client))
        } else {
            let pipeline = self
                .config
                .build_networked_pipeline(
                    &config.stages,
                    network_client.clone(),
                    Arc::clone(&consensus),
                    provider_factory.clone(),
                    &executor.clone(),
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                )
                .await?;

            (pipeline, EitherDownloader::Right(network_client))
        };

        let pipeline_events = pipeline.events();

        let initial_target = self.config.initial_pipeline_target(genesis_hash);
        let mut hooks = EngineHooks::new();

        let pruner_events = if let Some(prune_config) = prune_config {
            let mut pruner = PrunerBuilder::new(prune_config.clone())
                .max_reorg_depth(tree_config.max_reorg_depth() as usize)
                .prune_delete_limit(self.config.chain.prune_delete_limit)
                .build(provider_factory, snapshotter.highest_snapshot_receiver());

            let events = pruner.events();
            hooks.add(PruneHook::new(pruner, Box::new(executor.clone())));

            info!(target: "reth::cli", ?prune_config, "Pruner initialized");
            Either::Left(events)
        } else {
            Either::Right(stream::empty())
        };

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            client,
            pipeline,
            blockchain_db.clone(),
            Box::new(executor.clone()),
            Box::new(network.clone()),
            max_block,
            self.config.debug.continuous,
            payload_builder.clone(),
            initial_target,
            MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            consensus_engine_rx,
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            network.event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if self.config.debug.tip.is_none() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into)
        );
        executor.spawn_critical(
            "events task",
            events::handle_events(
                Some(network.clone()),
                Some(head.number),
                events,
                self.db.clone(),
            ),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.config.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = self.data_dir.jwt_path();
        let jwt_secret = self.config.rpc.auth_jwt_secret(default_jwt_path)?;

        // adjust rpc port numbers based on instance number
        self.config.adjust_instance_ports();

        // Start RPC servers
        let rpc_server_handles =
            self.config.rpc.start_servers(&components, engine_api, jwt_secret, &mut ext).await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        ext.on_node_started(&components)?;

        // If `enable_genesis_walkback` is set to true, the rollup client will need to
        // perform the derivation pipeline from genesis, validating the data dir.
        // When set to false, set the finalized, safe, and unsafe head block hashes
        // on the rollup client using a fork choice update. This prevents the rollup
        // client from performing the derivation pipeline from genesis, and instead
        // starts syncing from the current tip in the DB.
        #[cfg(feature = "optimism")]
        if self.config.chain.is_optimism() && !self.config.rollup.enable_genesis_walkback {
            let client = rpc_server_handles.auth.http_client();
            reth_rpc_api::EngineApiClient::fork_choice_updated_v2(
                &client,
                reth_rpc_types::engine::ForkchoiceState {
                    head_block_hash: head.hash,
                    safe_block_hash: head.hash,
                    finalized_block_hash: head.hash,
                },
                None,
            )
            .await?;
        }

        // construct node handle and return
        let node_handle = NodeHandle {
            rpc_server_handles,
            consensus_engine_rx: rx,
            terminate: self.config.debug.terminate,
        };
        Ok(node_handle)
    }

    /// Returns the path to the config file.
    fn config_path(&self) -> PathBuf {
        self.config.config.clone().unwrap_or_else(|| self.data_dir.config_path())
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self) -> eyre::Result<Config> {
        let config_path = self.config_path();

        let mut config = confy::load_path::<Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        config.peers.connect_trusted_nodes_only = self.config.network.trusted_only;

        if !self.config.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.config.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        Ok(config)
    }
}

/// The [NodeHandle] contains the [RethRpcServerHandles] returned by the reth initialization
/// process, as well as a method for waiting for the node exit.
#[derive(Debug)]
pub struct NodeHandle {
    /// The handles to the RPC servers
    rpc_server_handles: RethRpcServerHandles,

    /// The receiver half of the channel for the consensus engine.
    /// This can be used to wait for the consensus engine to exit.
    consensus_engine_rx: oneshot::Receiver<Result<(), BeaconConsensusEngineError>>,

    /// Flag indicating whether the node should be terminated after the pipeline sync.
    terminate: bool,
}

impl NodeHandle {
    /// Returns the [RethRpcServerHandles] for this node.
    pub fn rpc_server_handles(&self) -> &RethRpcServerHandles {
        &self.rpc_server_handles
    }

    /// Waits for the node to exit, if it was configured to exit.
    pub async fn wait_for_node_exit(self) -> eyre::Result<()> {
        self.consensus_engine_rx.await??;
        info!(target: "reth::cli", "Consensus engine has exited.");

        if self.terminate {
            Ok(())
        } else {
            // The pipeline has finished downloading blocks up to `--debug.tip` or
            // `--debug.max-block`. Keep other node components alive for further usage.
            futures::future::pending().await
        }
    }
}

/// A simple function to launch a node with the specified [NodeConfig], spawning tasks on the
/// [TaskExecutor] constructed from [Handle::current].
///
/// # Example
/// ```
/// # use reth::{
/// #     builder::{NodeConfig, spawn_node},
/// #     args::RpcServerArgs,
/// # };
/// async fn t() {
///     // Create a node builder with an http rpc server enabled
///     let rpc_args = RpcServerArgs::default().with_http();
///
///     /// Set the node instance number to 2
///     let builder = NodeConfig::test().with_rpc(rpc_args).with_instance(2);
///
///     // Spawn the builder, returning a handle to the node
///     let _handle = spawn_node(builder).await.unwrap();
/// }
/// ```
pub async fn spawn_node(config: NodeConfig) -> eyre::Result<NodeHandle> {
    let handle = Handle::current();
    let task_manager = TaskManager::new(handle);
    let ext = DefaultRethNodeCommandConfig;
    config.launch::<()>(ext, task_manager.executor()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::U256;
    use reth_rpc_api::EthApiClient;

    #[tokio::test]
    async fn block_number_node_config_test() {
        // this launches a test node with http
        let rpc_args = RpcServerArgs::default().with_http();

        // NOTE: tests here manually set an instance number. The alternative would be to use an
        // atomic counter. This works for `cargo test` but if tests would be run in `nextest` then
        // they would become flaky. So new tests should manually set a unique instance number.
        let handle =
            spawn_node(NodeConfig::test().with_rpc(rpc_args).with_instance(1)).await.unwrap();

        // call a function on the node
        let client = handle.rpc_server_handles().rpc.http_client().unwrap();
        let block_number = client.block_number().await.unwrap();

        // it should be zero, since this is an ephemeral test node
        assert_eq!(block_number, U256::ZERO);
    }

    #[tokio::test]
    async fn rpc_handles_none_without_http() {
        // this launches a test node _without_ http
        let handle = spawn_node(NodeConfig::test().with_instance(2)).await.unwrap();

        // ensure that the `http_client` is none
        let maybe_client = handle.rpc_server_handles().rpc.http_client();
        assert!(maybe_client.is_none());
    }

    #[tokio::test]
    async fn launch_multiple_nodes() {
        // spawn_test_node takes roughly 1 second per node, so this test takes ~4 seconds
        let num_nodes = 4;

        let starting_instance = 3;
        let mut handles = Vec::new();
        for i in 0..num_nodes {
            let handle =
                spawn_node(NodeConfig::test().with_instance(starting_instance + i)).await.unwrap();
            handles.push(handle);
        }
    }
}
