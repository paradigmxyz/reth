//! Support for customizing the node

use crate::{
    args::{
        get_secret_key, DatabaseArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs,
        PruningArgs, RpcServerArgs, TxPoolArgs,
    },
    cli::{config::RethTransactionPoolConfig, db_type::DatabaseBuilder},
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
    metrics::prometheus_exporter,
    utils::{get_single_header, write_peers_to_file},
};
use metrics_exporter_prometheus::PrometheusHandle;
use once_cell::sync::Lazy;
use reth_auto_seal_consensus::{AutoSealConsensus, MiningMode};
use reth_beacon_consensus::BeaconConsensus;
use reth_blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree,
};
use reth_config::{
    config::{PruneConfig, StageConfig},
    Config,
};
use reth_db::{database::Database, database_metrics::DatabaseMetrics};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    blockchain_tree::BlockchainTreeEngine,
    consensus::Consensus,
    p2p::{
        bodies::{client::BodiesClient, downloader::BodyDownloader},
        headers::{client::HeadersClient, downloader::HeaderDownloader},
    },
    RethResult,
};
use reth_network::{
    transactions::{TransactionFetcherConfig, TransactionsManagerConfig},
    NetworkBuilder, NetworkConfig, NetworkHandle, NetworkManager,
};
use reth_node_api::ConfigureEvm;
use reth_primitives::{
    constants::eip4844::{LoadKzgSettingsError, MAINNET_KZG_TRUSTED_SETUP},
    kzg::KzgSettings,
    stage::StageId,
    BlockHashOrNumber, BlockNumber, ChainSpec, Head, SealedHeader, TxHash, B256, MAINNET,
};
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, BlockNumReader, BlockReader,
    BlockchainTreePendingStateProvider, CanonStateSubscriptions, HeaderProvider, HeaderSyncMode,
    ProviderFactory, StageCheckpointReader,
};
use reth_revm::EvmProcessorFactory;
use reth_stages::{
    prelude::*,
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, IndexAccountHistoryStage,
        IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TotalDifficultyStage, TransactionLookupStage,
    },
    MetricEvent,
};
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{
    blobstore::{DiskFileBlobStore, DiskFileBlobStoreConfig},
    EthTransactionPool, TransactionPool, TransactionValidationTaskExecutor,
};
use revm_inspectors::stack::Hook;
use secp256k1::SecretKey;
use std::{
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{
    mpsc::{Receiver, UnboundedSender},
    watch,
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
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
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
/// }
/// ```
///
/// This can also be used to launch a node with a temporary test database. This can be done with
/// the [NodeConfig::test] method.
///
/// # Example
/// ```rust
/// # use reth_tasks::{TaskManager, TaskSpawner};
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
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
/// }
/// ```
#[derive(Debug, Clone)]
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
        let mut test = Self {
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
        };

        // set all ports to zero by default for test instances
        test = test.with_unused_ports();
        test
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

    /// Set the [ChainSpec] for the node
    pub fn with_chain(mut self, chain: impl Into<Arc<ChainSpec>>) -> Self {
        self.chain = chain.into();
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

    /// Set the rollup args for the node
    #[cfg(feature = "optimism")]
    pub fn with_rollup(mut self, rollup: crate::args::RollupArgs) -> Self {
        self.rollup = rollup;
        self
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

    /// Returns pruning configuration.
    pub fn prune_config(&self) -> eyre::Result<Option<PruneConfig>> {
        self.pruning.prune_config(Arc::clone(&self.chain))
    }

    /// Returns the max block that the node should run to, looking it up from the network if
    /// necessary
    pub async fn max_block<Provider, Client>(
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

    /// Create the [NetworkBuilder].
    ///
    /// This only configures it and does not spawn it.
    pub async fn build_network<C>(
        &self,
        config: &Config,
        client: C,
        executor: TaskExecutor,
        head: Head,
        data_dir: &ChainPath<DataDirPath>,
    ) -> eyre::Result<NetworkBuilder<C, (), ()>>
    where
        C: BlockNumReader,
    {
        info!(target: "reth::cli", "Connecting to P2P network");
        let secret_key = self.network_secret(data_dir)?;
        let default_peers_path = data_dir.known_peers_path();
        let network_config = self.load_network_config(
            config,
            client,
            executor.clone(),
            head,
            secret_key,
            default_peers_path.clone(),
        );

        let builder = NetworkManager::builder(network_config).await?;
        Ok(builder)
    }

    /// Build the blockchain tree
    pub fn build_blockchain_tree<DB, EvmConfig>(
        &self,
        provider_factory: ProviderFactory<DB>,
        consensus: Arc<dyn Consensus>,
        prune_config: Option<PruneConfig>,
        sync_metrics_tx: UnboundedSender<MetricEvent>,
        tree_config: BlockchainTreeConfig,
        evm_config: EvmConfig,
    ) -> eyre::Result<BlockchainTree<DB, EvmProcessorFactory<EvmConfig>>>
    where
        DB: Database + Unpin + Clone + 'static,
        EvmConfig: ConfigureEvm + Clone + 'static,
    {
        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            provider_factory.clone(),
            consensus.clone(),
            EvmProcessorFactory::new(self.chain.clone(), evm_config),
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
    ) -> eyre::Result<EthTransactionPool<BlockchainProvider<DB, Tree>, DiskFileBlobStore>>
    where
        DB: Database + Unpin + Clone + 'static,
        Tree: BlockchainTreeEngine
            + BlockchainTreePendingStateProvider
            + CanonStateSubscriptions
            + Clone
            + 'static,
    {
        let blob_store = DiskFileBlobStore::open(
            data_dir.blobstore_path(),
            DiskFileBlobStoreConfig::default()
                .with_max_cached_entries(self.txpool.max_cached_entries),
        )?;
        let validator = TransactionValidationTaskExecutor::eth_builder(Arc::clone(&self.chain))
            .with_head_timestamp(head.timestamp)
            .kzg_settings(self.kzg_settings()?)
            // use an additional validation task so we can validate transactions in parallel
            .with_additional_tasks(1)
            // set the max tx size in bytes allowed to enter the pool
            .with_max_tx_input_bytes(self.txpool.max_tx_input_bytes)
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
    pub async fn build_networked_pipeline<DB, Client, EvmConfig>(
        &self,
        config: &StageConfig,
        client: Client,
        consensus: Arc<dyn Consensus>,
        provider_factory: ProviderFactory<DB>,
        task_executor: &TaskExecutor,
        metrics_tx: reth_stages::MetricEventsSender,
        prune_config: Option<PruneConfig>,
        max_block: Option<BlockNumber>,
        evm_config: EvmConfig,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Unpin + Clone + 'static,
        Client: HeadersClient + BodiesClient + Clone + 'static,
        EvmConfig: ConfigureEvm + Clone + 'static,
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
                evm_config,
            )
            .await?;

        Ok(pipeline)
    }

    /// Loads the trusted setup params from a given file path or falls back to
    /// `MAINNET_KZG_TRUSTED_SETUP`.
    pub fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        if let Some(ref trusted_setup_file) = self.trusted_setup_file {
            let trusted_setup = KzgSettings::load_trusted_setup_file(trusted_setup_file)
                .map_err(LoadKzgSettingsError::KzgError)?;
            Ok(Arc::new(trusted_setup))
        } else {
            Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
        }
    }

    /// Installs the prometheus recorder.
    pub fn install_prometheus_recorder(&self) -> eyre::Result<PrometheusHandle> {
        Ok(PROMETHEUS_RECORDER_HANDLE.clone())
    }

    /// Serves the prometheus endpoint over HTTP with the given database and prometheus handle.
    pub async fn start_metrics_endpoint<Metrics>(
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
    pub fn start_network<C, Pool>(
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
        let (handle, network, txpool, eth) = builder
            .transactions(
                pool, // Configure transactions manager
                TransactionsManagerConfig {
                    transaction_fetcher_config: TransactionFetcherConfig::new(
                        self.network.soft_limit_byte_size_pooled_transactions_response,
                        self.network
                            .soft_limit_byte_size_pooled_transactions_response_on_pack_request,
                    ),
                },
            )
            .request_handler(client)
            .split_with_handle();

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
    pub fn lookup_head<DB: Database>(&self, factory: ProviderFactory<DB>) -> RethResult<Head> {
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
    pub async fn lookup_or_fetch_tip<Provider, Client>(
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
    pub async fn fetch_tip_from_network<Client>(
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

    /// Builds the [NetworkConfig] with the given [ProviderFactory].
    pub fn load_network_config<C>(
        &self,
        config: &Config,
        client: C,
        executor: TaskExecutor,
        head: Head,
        secret_key: SecretKey,
        default_peers_path: PathBuf,
    ) -> NetworkConfig<C> {
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

        cfg_builder.build(client)
    }

    /// Builds the [Pipeline] with the given [ProviderFactory] and downloaders.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_pipeline<DB, H, B, EvmConfig>(
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
        evm_config: EvmConfig,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Clone + 'static,
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
        EvmConfig: ConfigureEvm + Clone + 'static,
    {
        let mut builder = Pipeline::builder();

        if let Some(max_block) = max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        use revm_inspectors::stack::InspectorStackConfig;
        let factory = reth_revm::EvmProcessorFactory::new(self.chain.clone(), evm_config);

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
                            max_duration: stage_config.execution.max_duration,
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
    pub fn adjust_instance_ports(&mut self) {
        self.rpc.adjust_instance_ports(self.instance);
    }

    /// Sets networking and RPC ports to zero, causing the OS to choose random unused ports when
    /// sockets are bound.
    pub fn with_unused_ports(mut self) -> Self {
        self.rpc = self.rpc.with_unused_ports();
        self.network = self.network.with_unused_ports();
        self
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
