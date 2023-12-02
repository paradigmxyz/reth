//! Support for customizing the node
use crate::{
    args::{
        get_secret_key, DatabaseArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs,
        PruningArgs, RpcServerArgs, TxPoolArgs,
    },
    cli::{
        components::RethNodeComponentsImpl,
        config::RethRpcConfig,
        ext::{RethCliExt, RethNodeCommandConfig},
    },
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
    init::init_genesis,
    node::{cl_events::ConsensusLayerHealthEvents, events, run_network_until_shutdown},
    prometheus_exporter,
    runner::tokio_runtime,
    utils::get_single_header,
    version::SHORT_VERSION,
};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{future::Either, stream, stream_select, StreamExt};
use metrics_exporter_prometheus::PrometheusHandle;
use reth_auto_seal_consensus::{AutoSealBuilder, AutoSealConsensus, MiningMode};
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook},
    BeaconConsensus, BeaconConsensusEngine, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree, ShareableBlockchainTree,
};
use reth_config::{
    config::{PruneConfig, StageConfig},
    Config,
};
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
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
    BlockHashOrNumber, BlockNumber, ChainSpec, DisplayHardforks, Head, SealedHeader, B256, MAINNET,
};
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, BlockReader, CanonStateSubscriptions,
    HeaderProvider, HeaderSyncMode, ProviderFactory, StageCheckpointReader,
};
use reth_prune::{segments::SegmentSet, Pruner};
use reth_revm::EvmProcessorFactory;
use reth_revm_inspectors::stack::Hook;
use reth_rpc_engine_api::EngineApi;
use reth_snapshot::HighestSnapshotsTracker;
use reth_stages::{
    prelude::*,
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, IndexAccountHistoryStage,
        IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TotalDifficultyStage, TransactionLookupStage,
    },
};
use reth_tasks::{TaskExecutor, TaskManager};
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, TransactionPool, TransactionValidationTaskExecutor,
};
use secp256k1::SecretKey;
use std::{
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot, watch};
use tracing::*;

use super::{
    components::RethRpcServerHandles, db_type::DatabaseType, ext::DefaultRethNodeCommandConfig,
};

/// Start the node
#[derive(Debug)]
pub struct NodeConfig {
    /// The test database
    pub database: DatabaseType,

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
            database: DatabaseType::default(),
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

    /// Launches the node, also adding any RPC extensions passed.
    ///
    /// # Example
    /// ```rust
    /// # use reth_tasks::TaskManager;
    /// fn t() {
    ///     use reth_tasks::TaskSpawner;
    ///     let rt = tokio::runtime::Runtime::new().unwrap();
    ///     let manager = TaskManager::new(rt.handle().clone());
    ///     let executor = manager.executor();
    ///     let config = NodeConfig::default();
    ///     let ext = DefaultRethNodeCommandConfig;
    ///     let handle = config.launch(ext, executor);
    /// }
    /// ```
    pub async fn launch<E: RethCliExt>(
        mut self,
        mut ext: E::Node,
        executor: TaskExecutor,
    ) -> eyre::Result<NodeHandle> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        // get config
        let config = self.load_config()?;

        let prometheus_handle = self.install_prometheus_recorder()?;

        let data_dir = self.data_dir().expect("see below");
        let db_path = data_dir.db_path();

        // TODO: set up test database, see if we can configure either real or test db
        // let db =
        //     DatabaseBuilder::new(self.database).build_db(self.db.log_level, self.chain.chain)?;
        // TODO: ok, this doesn't work because:
        // * Database is not object safe, and is sealed
        // * DatabaseInstance is not sealed
        // * ProviderFactory takes DB
        // * But this would make it ProviderFactory<DatabaseEnv> OR
        // ProviderFactory<TempDatabase<DatabaseEnv>>
        // * But we also have no Either<A: Database, B: Database> impl for
        // ProviderFactory<Either<A, B>> etc
        // * Because Database is not object safe we can't return Box<dyn Database> either
        // * EitherDatabase is nontrivial due to associated types
        let db = Arc::new(init_db(&db_path, self.db.log_level)?.with_metrics());
        info!(target: "reth::cli", "Database opened");

        let mut provider_factory = ProviderFactory::new(Arc::clone(&db), Arc::clone(&self.chain));

        // configure snapshotter
        let snapshotter = reth_snapshot::Snapshotter::new(
            provider_factory.clone(),
            data_dir.snapshots_path(),
            self.chain.snapshot_block_interval,
        )?;

        provider_factory = provider_factory
            .with_snapshots(data_dir.snapshots_path(), snapshotter.highest_snapshot_receiver());

        self.start_metrics_endpoint(prometheus_handle, Arc::clone(&db)).await?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(Arc::clone(&db), self.chain.clone())?;

        info!(target: "reth::cli", "{}", DisplayHardforks::new(self.chain.hardforks()));

        let consensus = self.consensus();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        let prune_config =
            self.pruning.prune_config(Arc::clone(&self.chain))?.or(config.prune.clone());

        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            provider_factory.clone(),
            Arc::clone(&consensus),
            EvmProcessorFactory::new(self.chain.clone()),
        );
        let tree_config = BlockchainTreeConfig::default();
        let tree = BlockchainTree::new(
            tree_externals,
            tree_config,
            prune_config.clone().map(|config| config.segments),
        )?
        .with_sync_metrics_tx(sync_metrics_tx.clone());
        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = ShareableBlockchainTree::new(tree);
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self.lookup_head(Arc::clone(&db)).wrap_err("the head block is missing")?;

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;
        let blob_store = InMemoryBlobStore::default();
        let validator = TransactionValidationTaskExecutor::eth_builder(Arc::clone(&self.chain))
            .with_head_timestamp(head.timestamp)
            .kzg_settings(self.kzg_settings()?)
            .with_additional_tasks(1)
            .build_with_tasks(blockchain_db.clone(), executor.clone(), blob_store.clone());

        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, self.txpool.pool_config());
        info!(target: "reth::cli", "Transaction pool initialized");

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = blockchain_db.canonical_state_stream();
            let client = blockchain_db.clone();
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

        info!(target: "reth::cli", "Connecting to P2P network");
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret_path());
        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;
        let default_peers_path = data_dir.known_peers_path();
        let network_config = self.load_network_config(
            &config,
            Arc::clone(&db),
            executor.clone(),
            head,
            secret_key,
            default_peers_path.clone(),
        );

        let network_client = network_config.client.clone();
        let mut network_builder = NetworkManager::builder(network_config).await?;

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
        let network = self.start_network(
            network_builder,
            &executor,
            transaction_pool.clone(),
            network_client,
            default_peers_path,
        );

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), enode = %network.local_node_record(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        ext.on_components_initialized(&components)?;

        debug!(target: "reth::cli", "Spawning payload builder service");
        let payload_builder = ext.spawn_payload_builder_service(&self.builder, &components)?;

        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();
        let max_block = if let Some(block) = self.debug.max_block {
            Some(block)
        } else if let Some(tip) = self.debug.tip {
            Some(self.lookup_or_fetch_tip(&db, &network_client, tip).await?)
        } else {
            None
        };

        // Configure the pipeline
        let (mut pipeline, client) = if self.dev.dev {
            info!(target: "reth::cli", "Starting Reth in dev mode");

            let mining_mode = if let Some(interval) = self.dev.block_time {
                MiningMode::interval(interval)
            } else if let Some(max_transactions) = self.dev.block_max_transactions {
                MiningMode::instant(
                    max_transactions,
                    transaction_pool.pending_transactions_listener(),
                )
            } else {
                info!(target: "reth::cli", "No mining mode specified, defaulting to ReadyTransaction");
                MiningMode::instant(1, transaction_pool.pending_transactions_listener())
            };

            let (_, client, mut task) = AutoSealBuilder::new(
                Arc::clone(&self.chain),
                blockchain_db.clone(),
                transaction_pool.clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
                mining_mode,
            )
            .build();

            let mut pipeline = self
                .build_networked_pipeline(
                    &config.stages,
                    client.clone(),
                    Arc::clone(&consensus),
                    provider_factory,
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
                .build_networked_pipeline(
                    &config.stages,
                    network_client.clone(),
                    Arc::clone(&consensus),
                    provider_factory,
                    &executor,
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                )
                .await?;

            (pipeline, EitherDownloader::Right(network_client))
        };

        let pipeline_events = pipeline.events();

        let initial_target = if let Some(tip) = self.debug.tip {
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
        };

        let mut hooks = EngineHooks::new();

        let pruner_events = if let Some(prune_config) = prune_config {
            let mut pruner = self.build_pruner(
                &prune_config,
                db.clone(),
                tree_config,
                snapshotter.highest_snapshot_receiver(),
            );

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
            self.debug.continuous,
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
            if self.debug.tip.is_none() {
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
            events::handle_events(Some(network.clone()), Some(head.number), events, db.clone()),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = data_dir.jwt_path();
        let jwt_secret = self.rpc.auth_jwt_secret(default_jwt_path)?;

        // adjust rpc port numbers based on instance number
        self.adjust_instance_ports();

        // Start RPC servers
        let rpc_server_handles =
            self.rpc.start_servers(&components, engine_api, jwt_secret, &mut ext).await?;

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
        if self.chain.is_optimism() && !self.rollup.enable_genesis_walkback {
            let client = _rpc_server_handles.auth.http_client();
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

        // we should return here
        let _node_handle = NodeHandle { _rpc_server_handles: rpc_server_handles };

        // TODO: we need a way to do this in the background because this method will just block
        // until the consensus engine exits
        //
        // We should probably return the node handle AND `rx` so the `execute` method can do all of
        // this stuff below, in the meantime we will spawn everything (analogous to foundry
        // spawning the NodeService).
        rx.await??;

        info!(target: "reth::cli", "Consensus engine has exited.");

        if self.debug.terminate {
            todo!()
        } else {
            // The pipeline has finished downloading blocks up to `--debug.tip` or
            // `--debug.max-block`. Keep other node components alive for further usage.
            futures::future::pending().await
        }
    }

    /// Set the datadir for the node
    pub fn datadir(mut self, datadir: MaybePlatformPath<DataDirPath>) -> Self {
        self.database = DatabaseType::Real(datadir);
        self
    }

    /// Set the config file for the node
    pub fn config(mut self, config: impl Into<PathBuf>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the chain for the node
    pub fn chain(mut self, chain: Arc<ChainSpec>) -> Self {
        self.chain = chain.into();
        self
    }

    /// Set the metrics address for the node
    pub fn metrics(mut self, metrics: SocketAddr) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the instance for the node
    pub fn instance(mut self, instance: u16) -> Self {
        self.instance = instance;
        self
    }

    /// Set the [Chain] for the node
    pub fn chain_spec(mut self, chain: Arc<ChainSpec>) -> Self {
        self.chain = chain;
        self
    }

    /// Set the trusted setup file for the node
    pub fn trusted_setup_file(mut self, trusted_setup_file: impl Into<PathBuf>) -> Self {
        self.trusted_setup_file = Some(trusted_setup_file.into());
        self
    }

    /// Set the network args for the node
    pub fn network(mut self, network: NetworkArgs) -> Self {
        self.network = network;
        self
    }

    /// Set the rpc args for the node
    pub fn rpc(mut self, rpc: RpcServerArgs) -> Self {
        self.rpc = rpc;
        self
    }

    /// Set the txpool args for the node
    pub fn txpool(mut self, txpool: TxPoolArgs) -> Self {
        self.txpool = txpool;
        self
    }

    /// Set the builder args for the node
    pub fn builder(mut self, builder: PayloadBuilderArgs) -> Self {
        self.builder = builder;
        self
    }

    /// Set the debug args for the node
    pub fn debug(mut self, debug: DebugArgs) -> Self {
        self.debug = debug;
        self
    }

    /// Set the database args for the node
    pub fn db(mut self, db: DatabaseArgs) -> Self {
        self.db = db;
        self
    }

    /// Set the dev args for the node
    pub fn dev(mut self, dev: DevArgs) -> Self {
        self.dev = dev;
        self
    }

    /// Set the pruning args for the node
    pub fn pruning(mut self, pruning: PruningArgs) -> Self {
        self.pruning = pruning;
        self
    }

    /// Set the rollup args for the node
    #[cfg(feature = "optimism")]
    pub fn rollup(mut self, rollup: crate::args::RollupArgs) -> Self {
        self.rollup = rollup;
        self
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
        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.bodies)
            .build(client, Arc::clone(&consensus), provider_factory.clone())
            .into_task_with(task_executor);

        let pipeline = self
            .build_pipeline(
                provider_factory,
                &config,
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

    /// Returns the chain specific path to the data dir. This returns `None` if the database is
    /// configured for testing.
    fn data_dir(&self) -> Option<ChainPath<DataDirPath>> {
        match &self.database {
            DatabaseType::Real(data_dir) => {
                Some(data_dir.unwrap_or_chain_default(self.chain.chain))
            }
            DatabaseType::Test => None,
        }
    }

    /// Returns the path to the config file.
    fn config_path(&self) -> Option<PathBuf> {
        let chain_dir = self.data_dir()?;

        let config = self.config.clone().unwrap_or_else(|| chain_dir.config_path());
        Some(config)
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self) -> eyre::Result<Config> {
        let Some(config_path) = self.config_path() else { todo!() };

        let mut config = confy::load_path::<Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        config.peers.connect_trusted_nodes_only = self.network.trusted_only;

        if !self.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        Ok(config)
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
        prometheus_exporter::install_recorder()
    }

    async fn start_metrics_endpoint(
        &self,
        prometheus_handle: PrometheusHandle,
        db: Arc<DatabaseEnv>,
    ) -> eyre::Result<()> {
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
        default_peers_path: PathBuf,
    ) -> NetworkHandle
    where
        C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
    {
        let (handle, network, txpool, eth) =
            builder.transactions(pool).request_handler(client).split_with_handle();

        task_executor.spawn_critical("p2p txpool", txpool);
        task_executor.spawn_critical("p2p eth request handler", eth);

        let known_peers_file = self.network.persistent_peers_file(default_peers_path);
        task_executor
            .spawn_critical_with_graceful_shutdown_signal("p2p network task", |shutdown| {
                run_network_until_shutdown(shutdown, network, known_peers_file)
            });

        handle
    }

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    fn lookup_head(&self, db: Arc<DatabaseEnv>) -> RethResult<Head> {
        let factory = ProviderFactory::new(db, self.chain.clone());
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
    async fn lookup_or_fetch_tip<DB, Client>(
        &self,
        db: DB,
        client: Client,
        tip: B256,
    ) -> RethResult<u64>
    where
        DB: Database,
        Client: HeadersClient,
    {
        Ok(self.fetch_tip(db, client, BlockHashOrNumber::Hash(tip)).await?.number)
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn fetch_tip<DB, Client>(
        &self,
        db: DB,
        client: Client,
        tip: BlockHashOrNumber,
    ) -> RethResult<SealedHeader>
    where
        DB: Database,
        Client: HeadersClient,
    {
        let factory = ProviderFactory::new(db, self.chain.clone());
        let provider = factory.provider()?;

        let header = provider.header_by_hash_or_number(tip)?;

        // try to look up the header in the database
        if let Some(header) = header {
            info!(target: "reth::cli", ?tip, "Successfully looked up tip block in the database");
            return Ok(header.seal_slow());
        }

        info!(target: "reth::cli", ?tip, "Fetching tip block from the network.");
        loop {
            match get_single_header(&client, tip).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?tip, "Successfully fetched tip");
                    return Ok(tip_header);
                }
                Err(error) => {
                    error!(target: "reth::cli", %error, "Failed to fetch the tip. Retrying...");
                }
            }
        }
    }

    fn load_network_config(
        &self,
        config: &Config,
        db: Arc<DatabaseEnv>,
        executor: TaskExecutor,
        head: Head,
        secret_key: SecretKey,
        default_peers_path: PathBuf,
    ) -> NetworkConfig<ProviderFactory<Arc<DatabaseEnv>>> {
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

        cfg_builder.build(ProviderFactory::new(db, self.chain.clone()))
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

    /// Builds a [Pruner] with the given config.
    fn build_pruner<DB: Database>(
        &self,
        config: &PruneConfig,
        db: DB,
        tree_config: BlockchainTreeConfig,
        highest_snapshots_rx: HighestSnapshotsTracker,
    ) -> Pruner<DB> {
        let segments = SegmentSet::default()
            // Receipts
            .segment_opt(config.segments.receipts.map(reth_prune::segments::Receipts::new))
            // Receipts by logs
            .segment_opt((!config.segments.receipts_log_filter.is_empty()).then(|| {
                reth_prune::segments::ReceiptsByLogs::new(
                    config.segments.receipts_log_filter.clone(),
                )
            }))
            // Transaction lookup
            .segment_opt(
                config
                    .segments
                    .transaction_lookup
                    .map(reth_prune::segments::TransactionLookup::new),
            )
            // Sender recovery
            .segment_opt(
                config.segments.sender_recovery.map(reth_prune::segments::SenderRecovery::new),
            )
            // Account history
            .segment_opt(
                config.segments.account_history.map(reth_prune::segments::AccountHistory::new),
            )
            // Storage history
            .segment_opt(
                config.segments.storage_history.map(reth_prune::segments::StorageHistory::new),
            );

        Pruner::new(
            db,
            self.chain.clone(),
            segments.into_vec(),
            config.block_interval,
            self.chain.prune_delete_limit,
            tree_config.max_reorg_depth() as usize,
            highest_snapshots_rx,
        )
    }

    /// Change rpc port numbers based on the instance number.
    fn adjust_instance_ports(&mut self) {
        // auth port is scaled by a factor of instance * 100
        self.rpc.auth_port += self.instance * 100 - 100;
        // http port is scaled by a factor of -instance
        self.rpc.http_port -= self.instance - 1;
        // ws port is scaled by a factor of instance * 2
        self.rpc.ws_port += self.instance * 2 - 2;
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            database: DatabaseType::default(),
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

/// The node handle
// We need from each component on init:
// * channels (for wiring into other components)
// * handles (for wiring into other components)
//   * also for giving to the NodeHandle, for example everything rpc
#[derive(Debug)]
pub struct NodeHandle {
    /// The handles to the RPC servers
    _rpc_server_handles: RethRpcServerHandles,
}

/// The node service
#[derive(Debug)]
pub struct NodeService;

/// A simple function to launch a node with the specified [NodeConfig], spawning tasks on the
/// [TaskExecutor] constructed from [tokio_runtime].
pub async fn spawn_node(config: NodeConfig) -> eyre::Result<NodeHandle> {
    let runtime = tokio_runtime()?;
    let task_manager = TaskManager::new(runtime.handle().clone());
    let ext = DefaultRethNodeCommandConfig;
    config.launch::<()>(ext, task_manager.executor()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_config() {
        // we need to override the db
        let _handle = spawn_node(NodeConfig::default()).await;
    }
}
