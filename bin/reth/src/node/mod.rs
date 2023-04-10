//! Main node command
//!
//! Starts the client
use crate::{
    args::{get_secret_key, DebugArgs, NetworkArgs, RpcServerArgs},
    dirs::{ConfigPath, DbPath, PlatformPath, SecretKeyPath},
    prometheus_exporter,
    runner::CliContext,
    utils::get_single_header,
};
use clap::{crate_version, Parser};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{pin_mut, stream::select as stream_select, FutureExt, StreamExt};
use reth_auto_seal_consensus::{AutoSealBuilder, AutoSealConsensus};
use reth_beacon_consensus::{BeaconConsensus, BeaconConsensusEngine, BeaconEngineMessage};
use reth_db::{
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::DbTx,
};
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_executor::blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree, ShareableBlockchainTree,
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    events::NewBlockNotificationSink,
    p2p::{
        bodies::{client::BodiesClient, downloader::BodyDownloader},
        headers::{client::StatusUpdater, downloader::HeaderDownloader},
    },
    sync::SyncStateUpdater,
};
use reth_miner::TestPayloadStore;
use reth_network::{error::NetworkError, NetworkConfig, NetworkHandle, NetworkManager};
use reth_network_api::NetworkInfo;
use reth_primitives::{BlockHashOrNumber, ChainSpec, Head, Header, SealedHeader, H256};
use reth_provider::{BlockProvider, HeaderProvider, ShareableDatabase};
use reth_revm::Factory;
use reth_revm_inspectors::stack::Hook;
use reth_rpc_engine_api::{EngineApi, EngineApiHandle};
use reth_staged_sync::{
    utils::{
        chainspec::genesis_value_parser,
        init::{init_db, init_genesis},
        parse_socket_address,
    },
    Config,
};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, HeaderSyncMode, SenderRecoveryStage, TotalDifficultyStage, FINISH},
};
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{EthTransactionValidator, TransactionPool};
use secp256k1::SecretKey;
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedSender},
    oneshot, watch,
};
use tracing::*;

use reth_interfaces::p2p::headers::client::HeadersClient;
use reth_stages::stages::{MERKLE_EXECUTION, MERKLE_UNWIND};

pub mod events;

/// Start the node
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: PlatformPath<ConfigPath>,

    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: PlatformPath<DbPath>,

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

    /// Secret key to use for this node.
    ///
    /// This also will deterministically set the peer ID.
    #[arg(long, value_name = "PATH", global = true, required = false, default_value_t)]
    p2p_secret_key: PlatformPath<SecretKeyPath>,

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
    debug: DebugArgs,

    /// Automatically mine blocks for new transactions
    #[arg(long)]
    auto_mine: bool,
}

impl Command {
    /// Execute `node` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", crate_version!());

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        let mut config: Config = self.load_config()?;
        info!(target: "reth::cli", path = %self.config, "Configuration loaded");

        info!(target: "reth::cli", path = %self.db, "Opening database");
        let db = Arc::new(init_db(&self.db)?);
        let shareable_db = ShareableDatabase::new(Arc::clone(&db), Arc::clone(&self.chain));
        info!(target: "reth::cli", "Database opened");

        self.start_metrics_endpoint(Arc::clone(&db)).await?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        init_genesis(db.clone(), self.chain.clone())?;

        let consensus: Arc<dyn Consensus> = if self.auto_mine {
            debug!(target: "reth::cli", "Using auto seal");
            Arc::new(AutoSealConsensus::new(Arc::clone(&self.chain)))
        } else {
            Arc::new(BeaconConsensus::new(Arc::clone(&self.chain)))
        };

        self.init_trusted_nodes(&mut config);

        let transaction_pool = reth_transaction_pool::Pool::eth_pool(
            EthTransactionValidator::new(shareable_db.clone(), Arc::clone(&self.chain)),
            Default::default(),
        );
        info!(target: "reth::cli", "Test transaction pool initialized");

        info!(target: "reth::cli", "Connecting to P2P network");
        let secret_key = get_secret_key(&self.p2p_secret_key)?;
        let network_config = self.load_network_config(
            &config,
            Arc::clone(&db),
            ctx.task_executor.clone(),
            secret_key,
        );
        let network = self
            .start_network(network_config, &ctx.task_executor, transaction_pool.clone())
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");

        if self.debug.continuous {
            info!(target: "reth::cli", "Continuous sync mode enabled");
        }

        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();

        // Forward the `debug.tip` as forkchoice state to the consensus engine.
        // This will initiate the sync up to the provided tip.
        let _tip_rx = match self.debug.tip {
            Some(tip) => {
                let (tip_tx, tip_rx) = oneshot::channel();
                let state = ForkchoiceState {
                    head_block_hash: tip,
                    finalized_block_hash: tip,
                    safe_block_hash: tip,
                };
                consensus_engine_tx.send(BeaconEngineMessage::ForkchoiceUpdated {
                    state,
                    payload_attrs: None,
                    tx: tip_tx,
                })?;
                debug!(target: "reth::cli", %tip, "Tip manually set");
                Some(tip_rx)
            }
            None => {
                let warn_msg = "No tip specified. \
                reth cannot communicate with consensus clients, \
                so a tip must manually be provided for the online stages with --debug.tip <HASH>.";
                warn!(target: "reth::cli", warn_msg);
                None
            }
        };

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
        let new_block_notification_sender =
            NewBlockNotificationSink::new(tree_config.max_reorg_depth() as usize * 2);
        let blockchain_tree = ShareableBlockchainTree::new(BlockchainTree::new(
            tree_externals,
            new_block_notification_sender.clone(),
            tree_config,
        )?);

        // Configure the pipeline
        let mut pipeline = if self.auto_mine {
            let (_, client, mut task) = AutoSealBuilder::new(
                Arc::clone(&self.chain),
                shareable_db.clone(),
                transaction_pool.clone(),
                consensus_engine_tx.clone(),
                new_block_notification_sender.clone(),
            )
            .build();

            let mut pipeline = self
                .build_networked_pipeline(
                    &mut config,
                    network.clone(),
                    client,
                    Arc::clone(&consensus),
                    db.clone(),
                    &ctx.task_executor,
                )
                .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            ctx.task_executor.spawn(Box::pin(task));

            pipeline
        } else {
            let client = network.fetch_client().await?;
            self.build_networked_pipeline(
                &mut config,
                network.clone(),
                client,
                Arc::clone(&consensus),
                db.clone(),
                &ctx.task_executor,
            )
            .await?
        };

        let events = stream_select(
            network.event_listener().map(Into::into),
            pipeline.events().map(Into::into),
        );
        ctx.task_executor
            .spawn_critical("events task", events::handle_events(Some(network.clone()), events));

        // TODO: change to non-test or rename this component eventually
        let test_payload_store = TestPayloadStore::default();

        let beacon_consensus_engine = BeaconConsensusEngine::new(
            Arc::clone(&db),
            ctx.task_executor.clone(),
            pipeline,
            blockchain_tree.clone(),
            consensus_engine_rx,
            self.debug.max_block,
            test_payload_store,
        );
        info!(target: "reth::cli", "Consensus engine initialized");

        let engine_api_handle =
            self.init_engine_api(Arc::clone(&db), consensus_engine_tx.clone(), &ctx.task_executor);
        info!(target: "reth::cli", "Engine API handler initialized");

        let launch_rpc = self
            .rpc
            .start_rpc_server(
                shareable_db.clone(),
                transaction_pool.clone(),
                network.clone(),
                ctx.task_executor.clone(),
                blockchain_tree,
            )
            .inspect(|_| {
                info!(target: "reth::cli", "Started RPC server");
            });

        let launch_auth = self
            .rpc
            .start_auth_server(
                shareable_db.clone(),
                transaction_pool.clone(),
                network.clone(),
                ctx.task_executor.clone(),
                self.chain.clone(),
                engine_api_handle,
            )
            .inspect(|_| {
                info!(target: "reth::cli", "Started Auth server");
            });

        // launch servers
        let (_rpc_server, _auth_server) =
            futures::future::try_join(launch_rpc, launch_auth).await?;

        // Run consensus engine to completion
        let (rx, tx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = rx.send(res);
        });

        tx.await??;

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
    async fn build_networked_pipeline<Client>(
        &self,
        config: &mut Config,
        network: NetworkHandle,
        client: Client,
        consensus: Arc<dyn Consensus>,
        db: Arc<Env<WriteMap>>,
        task_executor: &TaskExecutor,
    ) -> eyre::Result<Pipeline<Env<WriteMap>, NetworkHandle>>
    where
        Client: HeadersClient + BodiesClient + Clone + 'static,
    {
        let max_block = if let Some(block) = self.debug.max_block {
            Some(block)
        } else if let Some(tip) = self.debug.tip {
            Some(self.lookup_or_fetch_tip(db.clone(), &client, tip).await?)
        } else {
            None
        };

        // building network downloaders using the fetch client
        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(client, Arc::clone(&consensus), db.clone())
            .into_task_with(task_executor);

        let pipeline = self
            .build_pipeline(
                config,
                header_downloader,
                body_downloader,
                network.clone(),
                consensus,
                max_block,
                self.debug.continuous,
            )
            .await?;

        Ok(pipeline)
    }

    fn load_config(&self) -> eyre::Result<Config> {
        confy::load_path::<Config>(&self.config).wrap_err_with(|| {
            format!("Could not load config file {}", self.config.as_ref().display())
        })
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

    async fn start_metrics_endpoint(&self, db: Arc<Env<WriteMap>>) -> eyre::Result<()> {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");

            prometheus_exporter::initialize_with_db_metrics(listen_addr, db).await?;
        }

        Ok(())
    }

    fn init_engine_api(
        &self,
        db: Arc<Env<WriteMap>>,
        engine_tx: UnboundedSender<BeaconEngineMessage>,
        task_executor: &TaskExecutor,
    ) -> EngineApiHandle {
        let (message_tx, message_rx) = unbounded_channel();
        let engine_api = EngineApi::new(
            ShareableDatabase::new(db, self.chain.clone()),
            self.chain.clone(),
            message_rx,
            engine_tx,
        );
        task_executor.spawn_critical("engine API task", engine_api);
        message_tx
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    async fn start_network<C, Pool>(
        &self,
        config: NetworkConfig<C>,
        task_executor: &TaskExecutor,
        pool: Pool,
    ) -> Result<NetworkHandle, NetworkError>
    where
        C: BlockProvider + HeaderProvider + Clone + Unpin + 'static,
        Pool: TransactionPool + Unpin + 'static,
    {
        let client = config.client.clone();
        let (handle, network, txpool, eth) = NetworkManager::builder(config)
            .await?
            .transactions(pool)
            .request_handler(client)
            .split_with_handle();

        let known_peers_file = self.network.persistent_peers_file();
        task_executor.spawn_critical_with_signal("p2p network task", |shutdown| {
            run_network_until_shutdown(shutdown, network, known_peers_file)
        });

        task_executor.spawn_critical("p2p eth request handler", eth);
        task_executor.spawn_critical("p2p txpool request handler", txpool);

        Ok(handle)
    }

    fn lookup_head(&self, db: Arc<Env<WriteMap>>) -> Result<Head, reth_interfaces::db::Error> {
        db.view(|tx| {
            let head = FINISH.get_progress(tx)?.unwrap_or_default();
            let header = tx
                .get::<tables::Headers>(head)?
                .expect("the header for the latest block is missing, database is corrupt");
            let total_difficulty = tx.get::<tables::HeaderTD>(head)?.expect(
                "the total difficulty for the latest block is missing, database is corrupt",
            );
            let hash = tx
                .get::<tables::CanonicalHeaders>(head)?
                .expect("the hash for the latest block is missing, database is corrupt");
            Ok::<Head, reth_interfaces::db::Error>(Head {
                number: head,
                hash,
                difficulty: header.difficulty,
                total_difficulty: total_difficulty.into(),
                timestamp: header.timestamp,
            })
        })?
        .map_err(Into::into)
    }

    /// Attempt to look up the block number for the tip hash in the database.
    /// If it doesn't exist, download the header and return the block number.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn lookup_or_fetch_tip<Client>(
        &self,
        db: Arc<Env<WriteMap>>,
        client: Client,
        tip: H256,
    ) -> Result<u64, reth_interfaces::Error>
    where
        Client: HeadersClient,
    {
        Ok(self.fetch_tip(db, client, BlockHashOrNumber::Hash(tip)).await?.number)
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn fetch_tip<Client>(
        &self,
        db: Arc<Env<WriteMap>>,
        client: Client,
        tip: BlockHashOrNumber,
    ) -> Result<SealedHeader, reth_interfaces::Error>
    where
        Client: HeadersClient,
    {
        let header = db.view(|tx| -> Result<Option<Header>, reth_db::Error> {
            let number = match tip {
                BlockHashOrNumber::Hash(hash) => tx.get::<tables::HeaderNumbers>(hash)?,
                BlockHashOrNumber::Number(number) => Some(number),
            };
            Ok(number.map(|number| tx.get::<tables::Headers>(number)).transpose()?.flatten())
        })??;

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
        db: Arc<Env<WriteMap>>,
        executor: TaskExecutor,
        secret_key: SecretKey,
    ) -> NetworkConfig<ShareableDatabase<Arc<Env<WriteMap>>>> {
        let head = self.lookup_head(Arc::clone(&db)).expect("the head block is missing");

        self.network
            .network_config(config, self.chain.clone(), secret_key)
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
            .build(ShareableDatabase::new(db, self.chain.clone()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_pipeline<H, B, U>(
        &self,
        config: &Config,
        header_downloader: H,
        body_downloader: B,
        updater: U,
        consensus: Arc<dyn Consensus>,
        max_block: Option<u64>,
        continuous: bool,
    ) -> eyre::Result<Pipeline<Env<WriteMap>, U>>
    where
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
        U: SyncStateUpdater + StatusUpdater + Clone + 'static,
    {
        let stage_conf = &config.stages;

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
            .with_sync_state_updater(updater.clone())
            .with_tip_sender(tip_tx)
            .add_stages(
                DefaultStages::new(
                    header_mode,
                    Arc::clone(&consensus),
                    header_downloader,
                    body_downloader,
                    updater,
                    factory.clone(),
                )
                .set(
                    TotalDifficultyStage::new(consensus)
                        .with_commit_threshold(stage_conf.total_difficulty.commit_threshold),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: stage_conf.sender_recovery.commit_threshold,
                })
                .set(ExecutionStage::new(factory, stage_conf.execution.commit_threshold))
                .disable_if(MERKLE_UNWIND, || self.auto_mine)
                .disable_if(MERKLE_EXECUTION, || self.auto_mine),
            )
            .build();

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
    C: BlockProvider + HeaderProvider + Clone + Unpin + 'static,
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
    use std::net::IpAddr;

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
        let cmd = Command::try_parse_from(["reth", "--metrics", "9000"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9000)));

        let cmd = Command::try_parse_from(["reth", "--metrics", ":9000"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9000)));

        let cmd = Command::try_parse_from(["reth", "--metrics", "localhost:9000"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9000)));
    }
}
