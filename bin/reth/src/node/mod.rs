//! Main node command
//!
//! Starts the client
use crate::{
    args::{get_secret_key, DebugArgs, NetworkArgs, RpcServerArgs},
    dirs::DataDirPath,
    prometheus_exporter,
    runner::CliContext,
    utils::get_single_header,
    version::SHORT_VERSION,
};
use clap::Parser;
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{future::Either, pin_mut, stream, stream_select, StreamExt};
use reth_auto_seal_consensus::{AutoSealBuilder, AutoSealConsensus};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_beacon_consensus::{BeaconConsensus, BeaconConsensusEngine, MIN_BLOCKS_FOR_PIPELINE_RUN};
use reth_blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree, ShareableBlockchainTree,
};
use reth_config::Config;
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        bodies::{client::BodiesClient, downloader::BodyDownloader},
        either::EitherDownloader,
        headers::downloader::HeaderDownloader,
    },
};
use reth_network::{error::NetworkError, NetworkConfig, NetworkHandle, NetworkManager};
use reth_network_api::NetworkInfo;
use reth_primitives::{
    stage::StageId, BlockHashOrNumber, BlockNumber, ChainSpec, Head, SealedHeader, H256,
};
use reth_provider::{
    BlockHashReader, BlockReader, CanonStateSubscriptions, HeaderProvider, ProviderFactory,
    StageCheckpointReader,
};
use reth_revm::Factory;
use reth_revm_inspectors::stack::Hook;
use reth_rpc_engine_api::EngineApi;
use reth_staged_sync::utils::init::init_genesis;
use reth_stages::{
    prelude::*,
    stages::{
        ExecutionStage, ExecutionStageThresholds, HeaderSyncMode, SenderRecoveryStage,
        TotalDifficultyStage,
    },
    MetricEventsSender, MetricsListener,
};
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{EthTransactionValidator, TransactionPool};
use secp256k1::SecretKey;
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot, watch};
use tracing::*;

use crate::{
    args::{
        utils::{genesis_value_parser, parse_socket_address},
        PayloadBuilderArgs,
    },
    dirs::MaybePlatformPath,
    node::cl_events::ConsensusLayerHealthEvents,
};
use reth_interfaces::p2p::headers::client::HeadersClient;
use reth_payload_builder::PayloadBuilderService;
use reth_primitives::DisplayHardforks;
use reth_provider::providers::BlockchainProvider;
use reth_stages::stages::{
    AccountHashingStage, IndexAccountHistoryStage, IndexStorageHistoryStage, MerkleStage,
    StorageHashingStage, TransactionLookupStage,
};

pub mod cl_events;
pub mod events;

/// Start the node
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
    metrics: Option<SocketAddr>,

    #[clap(flatten)]
    network: NetworkArgs,

    #[clap(flatten)]
    rpc: RpcServerArgs,

    #[clap(flatten)]
    builder: PayloadBuilderArgs,

    #[clap(flatten)]
    debug: DebugArgs,

    /// Automatically mine blocks for new transactions
    #[arg(long)]
    auto_mine: bool,
}

impl Command {
    /// Execute `node` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let config_path = self.config.clone().unwrap_or(data_dir.config_path());

        let mut config: Config = self.load_config(config_path.clone())?;

        // always store reth.toml in the data dir, not the chain specific data dir
        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        let db_path = data_dir.db_path();
        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(&db_path)?);
        info!(target: "reth::cli", "Database opened");

        self.start_metrics_endpoint(Arc::clone(&db)).await?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(db.clone(), self.chain.clone())?;

        info!(target: "reth::cli", "{}", DisplayHardforks::from(self.chain.hardforks().clone()));

        let consensus: Arc<dyn Consensus> = if self.auto_mine {
            debug!(target: "reth::cli", "Using auto seal");
            Arc::new(AutoSealConsensus::new(Arc::clone(&self.chain)))
        } else {
            Arc::new(BeaconConsensus::new(Arc::clone(&self.chain)))
        };

        self.init_trusted_nodes(&mut config);

        debug!(target: "reth::cli", "Spawning metrics listener task");
        let (metrics_tx, metrics_rx) = unbounded_channel();
        let metrics_listener = MetricsListener::new(metrics_rx);
        ctx.task_executor.spawn_critical("metrics listener task", metrics_listener);

        // configure blockchain tree
        let tree_externals = TreeExternals::new(
            db.clone(),
            Arc::clone(&consensus),
            Factory::new(self.chain.clone()),
            Arc::clone(&self.chain),
        );
        let tree_config = BlockchainTreeConfig::default();
        // The size of the broadcast is twice the maximum reorg depth, because at maximum reorg
        // depth at least N blocks must be sent at once.
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);
        let blockchain_tree = ShareableBlockchainTree::new(
            BlockchainTree::new(
                tree_externals,
                canon_state_notification_sender.clone(),
                tree_config,
            )?
            .with_sync_metrics_tx(metrics_tx.clone()),
        );

        // setup the blockchain provider
        let factory = ProviderFactory::new(Arc::clone(&db), Arc::clone(&self.chain));
        let blockchain_db = BlockchainProvider::new(factory, blockchain_tree.clone())?;

        let transaction_pool = reth_transaction_pool::Pool::eth_pool(
            EthTransactionValidator::new(
                blockchain_db.clone(),
                Arc::clone(&self.chain),
                ctx.task_executor.clone(),
                1,
            ),
            Default::default(),
        );
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
        let head = self.lookup_head(Arc::clone(&db)).expect("the head block is missing");
        let network_config = self.load_network_config(
            &config,
            Arc::clone(&db),
            ctx.task_executor.clone(),
            head,
            secret_key,
            default_peers_path.clone(),
        );
        let network = self
            .start_network(
                network_config,
                &ctx.task_executor,
                transaction_pool.clone(),
                default_peers_path,
            )
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();

        let payload_generator = BasicPayloadJobGenerator::new(
            blockchain_db.clone(),
            transaction_pool.clone(),
            ctx.task_executor.clone(),
            BasicPayloadJobGeneratorConfig::default()
                .interval(self.builder.interval)
                .deadline(self.builder.deadline)
                .max_payload_tasks(self.builder.max_payload_tasks)
                .extradata(self.builder.extradata_bytes())
                .max_gas_limit(self.builder.max_gas_limit),
            Arc::clone(&self.chain),
        );
        let (payload_service, payload_builder) = PayloadBuilderService::new(payload_generator);

        debug!(target: "reth::cli", "Spawning payload builder service");
        ctx.task_executor.spawn_critical("payload builder service", payload_service);

        let max_block = if let Some(block) = self.debug.max_block {
            Some(block)
        } else if let Some(tip) = self.debug.tip {
            Some(self.lookup_or_fetch_tip(&db, &network_client, tip).await?)
        } else {
            None
        };

        // Configure the pipeline
        let (mut pipeline, client) = if self.auto_mine {
            let (_, client, mut task) = AutoSealBuilder::new(
                Arc::clone(&self.chain),
                blockchain_db.clone(),
                transaction_pool.clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
            )
            .build();

            let mut pipeline = self
                .build_networked_pipeline(
                    &mut config,
                    client.clone(),
                    Arc::clone(&consensus),
                    db.clone(),
                    &ctx.task_executor,
                    metrics_tx,
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
                    &mut config,
                    network_client.clone(),
                    Arc::clone(&consensus),
                    db.clone(),
                    &ctx.task_executor,
                    metrics_tx,
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
            }
        );
        ctx.task_executor.spawn_critical(
            "events task",
            events::handle_events(Some(network.clone()), Some(head.number), events),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = data_dir.jwt_path();
        let jwt_secret = self.rpc.jwt_secret(default_jwt_path)?;

        // Start RPC servers
        let (_rpc_server, _auth_server) = self
            .rpc
            .start_servers(
                blockchain_db.clone(),
                transaction_pool.clone(),
                network.clone(),
                ctx.task_executor.clone(),
                blockchain_tree,
                engine_api,
                jwt_secret,
            )
            .await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        rx.await??;

        info!(target: "reth::cli", "Consensus engine has exited.");

        if self.debug.terminate {
            Ok(())
        } else {
            // The pipeline has finished downloading blocks up to `--debug.tip` or
            // `--debug.max-block`. Keep other node components alive for further usage.
            futures::future::pending().await
        }
    }

    /// Constructs a [Pipeline] that's wired to the network
    #[allow(clippy::too_many_arguments)]
    async fn build_networked_pipeline<DB, Client>(
        &self,
        config: &mut Config,
        client: Client,
        consensus: Arc<dyn Consensus>,
        db: DB,
        task_executor: &TaskExecutor,
        metrics_tx: MetricEventsSender,
        max_block: Option<BlockNumber>,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Unpin + Clone + 'static,
        Client: HeadersClient + BodiesClient + Clone + 'static,
    {
        // building network downloaders using the fetch client
        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(client, Arc::clone(&consensus), db.clone())
            .into_task_with(task_executor);

        let pipeline = self
            .build_pipeline(
                db,
                config,
                header_downloader,
                body_downloader,
                consensus,
                max_block,
                self.debug.continuous,
                metrics_tx,
            )
            .await?;

        Ok(pipeline)
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self, config_path: PathBuf) -> eyre::Result<Config> {
        confy::load_path::<Config>(config_path.clone())
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))
    }

    fn init_trusted_nodes(&self, config: &mut Config) {
        config.peers.connect_trusted_nodes_only = self.network.trusted_only;

        if !self.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }
    }

    async fn start_metrics_endpoint(&self, db: Arc<DatabaseEnv>) -> eyre::Result<()> {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::initialize(listen_addr, db, metrics_process::Collector::default())
                .await?;
        }

        Ok(())
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    async fn start_network<C, Pool>(
        &self,
        config: NetworkConfig<C>,
        task_executor: &TaskExecutor,
        pool: Pool,
        default_peers_path: PathBuf,
    ) -> Result<NetworkHandle, NetworkError>
    where
        C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
    {
        let client = config.client.clone();
        let (handle, network, txpool, eth) = NetworkManager::builder(config)
            .await?
            .transactions(pool)
            .request_handler(client)
            .split_with_handle();

        task_executor.spawn_critical("p2p txpool", txpool);
        task_executor.spawn_critical("p2p eth request handler", eth);

        let known_peers_file = self.network.persistent_peers_file(default_peers_path);
        task_executor.spawn_critical_with_signal("p2p network task", |shutdown| {
            run_network_until_shutdown(shutdown, network, known_peers_file)
        });

        Ok(handle)
    }

    fn lookup_head(&self, db: Arc<DatabaseEnv>) -> Result<Head, reth_interfaces::Error> {
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
        db: &DB,
        client: Client,
        tip: H256,
    ) -> Result<u64, reth_interfaces::Error>
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
        db: &DB,
        client: Client,
        tip: BlockHashOrNumber,
    ) -> Result<SealedHeader, reth_interfaces::Error>
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
            return Ok(header.seal_slow())
        }

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

    fn load_network_config(
        &self,
        config: &Config,
        db: Arc<DatabaseEnv>,
        executor: TaskExecutor,
        head: Head,
        secret_key: SecretKey,
        default_peers_path: PathBuf,
    ) -> NetworkConfig<ProviderFactory<Arc<DatabaseEnv>>> {
        self.network
            .network_config(config, self.chain.clone(), secret_key, default_peers_path)
            .with_task_executor(Box::new(executor))
            .set_head(head)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                self.network.port.unwrap_or(DEFAULT_DISCOVERY_PORT),
            )))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                self.network.discovery.port.unwrap_or(DEFAULT_DISCOVERY_PORT),
            )))
            .build(ProviderFactory::new(db, self.chain.clone()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_pipeline<DB, H, B>(
        &self,
        db: DB,
        config: &Config,
        header_downloader: H,
        body_downloader: B,
        consensus: Arc<dyn Consensus>,
        max_block: Option<u64>,
        continuous: bool,
        metrics_tx: MetricEventsSender,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Clone + 'static,
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
    {
        let stage_config = &config.stages;

        let mut builder = Pipeline::builder();

        if let Some(max_block) = max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let (tip_tx, tip_rx) = watch::channel(H256::zero());
        use reth_revm_inspectors::stack::InspectorStackConfig;
        let factory = reth_revm::Factory::new(self.chain.clone());

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

        let header_mode =
            if continuous { HeaderSyncMode::Continuous } else { HeaderSyncMode::Tip(tip_rx) };
        let pipeline = builder
            .with_tip_sender(tip_tx)
            .with_metrics_tx(metrics_tx)
            .add_stages(
                DefaultStages::new(
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
                .set(ExecutionStage::new(
                    factory,
                    ExecutionStageThresholds {
                        max_blocks: stage_config.execution.max_blocks,
                        max_changes: stage_config.execution.max_changes,
                    },
                ))
                .set(AccountHashingStage::new(
                    stage_config.account_hashing.clean_threshold,
                    stage_config.account_hashing.commit_threshold,
                ))
                .set(StorageHashingStage::new(
                    stage_config.storage_hashing.clean_threshold,
                    stage_config.storage_hashing.commit_threshold,
                ))
                .set(MerkleStage::new_execution(stage_config.merkle.clean_threshold))
                .set(TransactionLookupStage::new(stage_config.transaction_lookup.commit_threshold))
                .set(IndexAccountHistoryStage::new(
                    stage_config.index_account_history.commit_threshold,
                ))
                .set(IndexStorageHistoryStage::new(
                    stage_config.index_storage_history.commit_threshold,
                )),
            )
            .build(db, self.chain.clone());

        Ok(pipeline)
    }
}

/// Drives the [NetworkManager] future until a [Shutdown](reth_tasks::shutdown::Shutdown) signal is
/// received. If configured, this writes known peers to `persistent_peers_file` afterwards.
async fn run_network_until_shutdown<C>(
    shutdown: reth_tasks::shutdown::Shutdown,
    network: NetworkManager<C>,
    persistent_peers_file: Option<PathBuf>,
) where
    C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
{
    pin_mut!(network, shutdown);

    tokio::select! {
        _ = &mut network => {},
        _ = shutdown => {},
    }

    if let Some(file_path) = persistent_peers_file {
        let known_peers = network.all_peers().collect::<Vec<_>>();
        if let Ok(known_peers) = serde_json::to_string_pretty(&known_peers) {
            trace!(target : "reth::cli", peers_file =?file_path, num_peers=%known_peers.len(), "Saving current peers");
            let parent_dir = file_path.parent().map(std::fs::create_dir_all).transpose();
            match parent_dir.and_then(|_| std::fs::write(&file_path, known_peers)) {
                Ok(_) => {
                    info!(target: "reth::cli", peers_file=?file_path, "Wrote network peers to file");
                }
                Err(err) => {
                    warn!(target: "reth::cli", ?err, peers_file=?file_path, "Failed to write network peers to file");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::IpAddr, path::Path};

    #[test]
    fn parse_help_node_command() {
        let err = Command::try_parse_from(["reth", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn parse_common_node_command_chain_args() {
        for chain in ["mainnet", "sepolia", "goerli"] {
            let args: Command = Command::parse_from(["reth", "--chain", chain]);
            assert_eq!(args.chain.chain, chain.parse().unwrap());
        }
    }

    #[test]
    fn parse_discovery_port() {
        let cmd = Command::try_parse_from(["reth", "--discovery.port", "300"]).unwrap();
        assert_eq!(cmd.network.discovery.port, Some(300));
    }

    #[test]
    fn parse_port() {
        let cmd =
            Command::try_parse_from(["reth", "--discovery.port", "300", "--port", "99"]).unwrap();
        assert_eq!(cmd.network.discovery.port, Some(300));
        assert_eq!(cmd.network.port, Some(99));
    }

    #[test]
    fn parse_metrics_port() {
        let cmd = Command::try_parse_from(["reth", "--metrics", "9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd = Command::try_parse_from(["reth", "--metrics", ":9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd = Command::try_parse_from(["reth", "--metrics", "localhost:9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));
    }

    #[test]
    fn parse_config_path() {
        let cmd = Command::try_parse_from(["reth", "--config", "my/path/to/reth.toml"]).unwrap();
        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let config_path = cmd.config.unwrap_or(data_dir.config_path());
        assert_eq!(config_path, Path::new("my/path/to/reth.toml"));

        let cmd = Command::try_parse_from(["reth"]).unwrap();

        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let config_path = cmd.config.clone().unwrap_or(data_dir.config_path());
        assert!(config_path.ends_with("reth/mainnet/reth.toml"), "{:?}", cmd.config);
    }

    #[test]
    fn parse_db_path() {
        let cmd = Command::try_parse_from(["reth"]).unwrap();
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let db_path = data_dir.db_path();
        assert!(db_path.ends_with("reth/mainnet/db"), "{:?}", cmd.config);

        let cmd = Command::try_parse_from(["reth", "--datadir", "my/custom/path"]).unwrap();
        let data_dir = cmd.datadir.unwrap_or_chain_default(cmd.chain.chain);
        let db_path = data_dir.db_path();
        assert_eq!(db_path, Path::new("my/custom/path/db"));
    }
}
