//! Support for customizing the node
use crate::{
    args::{
        DatabaseArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs, PruningArgs,
        RpcServerArgs, TxPoolArgs, get_secret_key,
    },
    cli::{RethCliExt, components::RethNodeComponentsImpl},
    version::SHORT_VERSION, init::init_genesis,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use fdlimit::raise_fd_limit;
use reth_db::init_db;
use reth_network::NetworkManager;
use reth_provider::ProviderFactory;
use tracing::{info, debug};
use reth_primitives::{
    ChainSpec, DisplayHardforks,
};


/// Start the node
#[derive(Debug)]
pub struct NodeConfig {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    pub datadir: PathBuf,

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
    /// Launches the node, also adding any RPC extensions passed.
    pub async fn launch<E: RethCliExt>(self, ext: E::Node) -> NodeHandle {
        // TODO: add easy way to handle auto seal, etc, without introducing generics
        // TODO: do we need ctx: CliContext here?
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        // get config
        let config = self.load_config()?;

        let prometheus_handle = self.install_prometheus_recorder()?;

        let data_dir = self.data_dir();
        let db_path = data_dir.db_path();

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        // TODO: set up test database, see if we can configure either real or test db
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
        ctx.task_executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

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
            .build_with_tasks(blockchain_db.clone(), ctx.task_executor.clone(), blob_store.clone());

        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, self.txpool.pool_config());
        info!(target: "reth::cli", "Transaction pool initialized");

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = blockchain_db.canonical_state_stream();
            let client = blockchain_db.clone();
            ctx.task_executor.spawn_critical(
                "txpool maintenance task",
                reth_transaction_pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    ctx.task_executor.clone(),
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
            ctx.task_executor.clone(),
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
            task_executor: ctx.task_executor.clone(),
            events: blockchain_db.clone(),
        };

        // allow network modifications
        self.ext.configure_network(network_builder.network_mut(), &components)?;

        // launch network
        let network = self.start_network(
            network_builder,
            &ctx.task_executor,
            transaction_pool.clone(),
            network_client,
            default_peers_path,
        );

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), enode = %network.local_node_record(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        self.ext.on_components_initialized(&components)?;

        debug!(target: "reth::cli", "Spawning payload builder service");
        let payload_builder = self.ext.spawn_payload_builder_service(&self.builder, &components)?;

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
                    &config,
                    client.clone(),
                    Arc::clone(&consensus),
                    provider_factory,
                    &ctx.task_executor,
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                )
                .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            ctx.task_executor.spawn(Box::pin(task));

            (pipeline, EitherDownloader::Left(client))
        } else {
            let pipeline = self
                .build_networked_pipeline(
                    &config,
                    network_client.clone(),
                    Arc::clone(&consensus),
                    provider_factory,
                    &ctx.task_executor,
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
            hooks.add(PruneHook::new(pruner, Box::new(ctx.task_executor.clone())));

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
            Box::new(ctx.task_executor.clone()),
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
        ctx.task_executor.spawn_critical(
            "events task",
            events::handle_events(Some(network.clone()), Some(head.number), events, db.clone()),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(ctx.task_executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = data_dir.jwt_path();
        let jwt_secret = self.rpc.auth_jwt_secret(default_jwt_path)?;

        // adjust rpc port numbers based on instance number
        self.adjust_instance_ports();

        // Start RPC servers
        let _rpc_server_handles =
            self.rpc.start_servers(&components, engine_api, jwt_secret, &mut self.ext).await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        self.ext.on_node_started(&components)?;

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

        rx.await??;

        info!(target: "reth::cli", "Consensus engine has exited.");

        if self.debug.terminate {
            Ok(())
        } else {
            // The pipeline has finished downloading blocks up to `--debug.tip` or
            // `--debug.max-block`. Keep other node components alive for further usage.
            futures::future::pending().await
        }
        todo!()
    }

    /// Set the datadir for the node
    pub fn datadir(mut self, datadir: impl Into<PathBuf>) -> Self {
        self.datadir = datadir.into();
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
}

/// The node handle
#[derive(Debug)]
pub struct NodeHandle;

#[cfg(test)]
mod tests {
    // use super::*;
    // use crate::args::NetworkArgs;
    // use reth_primitives::ChainSpec;
    // use std::net::Ipv4Addr;

    #[test]
    fn test_node_config() {

        // let config = NodeConfig::default()
        //     .datadir("/tmp")
        //     .config("/tmp/config.toml")
        //     .chain(Arc::new(ChainSpec::default()))
        //     .metrics(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8080))
        //     .instance(1)
        //     .trusted_setup_file("/tmp/trusted_setup")
        //     .network(NetworkArgs::default())
        //     .rpc(RpcServerArgs::default())
        //     .txpool(TxPoolArgs::default())
        //     .builder(PayloadBuilderArgs::default())
        //     .debug(DebugArgs::default())
        //     .db(DatabaseArgs::default())
        //     .dev(DevArgs::default())
        //     .pruning(PruningArgs::default());

        // assert_eq!(config.datadir, PathBuf::from("/tmp"));
        // assert_eq!(config.config, Some(PathBuf::from("/tmp/config.toml")));
        // assert_eq!(config.chain, Arc::new(ChainSpec::default()));
        // assert_eq!(config.metrics, Some(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8080)));
        // assert_eq!(config.instance, 1);
        // assert_eq!(config.trusted_setup_file, Some(PathBuf::from("/tmp/trusted_setup")));
        // assert_eq!(config.network, NetworkArgs::default());
        // assert_eq!(config.rpc, RpcServerArgs::default());
        // assert_eq!(config.txpool, TxPoolArgs::default());
        // assert_eq!(config.builder, PayloadBuilderArgs::default());
        // assert_eq!(config.debug, DebugArgs::default());
        // assert_eq!(config.db, DatabaseArgs::default());
        // assert_eq!(config.dev, DevArgs::default());
        // assert_eq!(config.pruning, PruningArgs::default());
    }
}
