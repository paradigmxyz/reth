//! Main node command
//!
//! Starts the client
use crate::{
    args::{NetworkArgs, RpcServerArgs},
    dirs::{ConfigPath, DbPath, PlatformPath},
    prometheus_exporter,
    runner::CliContext,
};
use clap::{crate_version, Parser};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{pin_mut, stream::select as stream_select, Stream, StreamExt};
use reth_consensus::beacon::BeaconConsensus;
use reth_db::{
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::DbTx,
};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    p2p::{
        bodies::downloader::BodyDownloader,
        headers::{client::StatusUpdater, downloader::HeaderDownloader},
    },
    sync::SyncStateUpdater,
};
use reth_network::{
    error::NetworkError, NetworkConfig, NetworkEvent, NetworkHandle, NetworkManager,
};
use reth_network_api::NetworkInfo;
use reth_primitives::{BlockNumber, ChainSpec, Head, H256};
use reth_provider::{BlockProvider, HeaderProvider, ShareableDatabase};
use reth_rpc_builder::{RethRpcModule, RpcServerConfig, TransportRpcModuleConfig};
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
    stages::{ExecutionStage, SenderRecoveryStage, TotalDifficultyStage, FINISH},
};
use reth_tasks::TaskExecutor;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tracing::{debug, info, trace, warn};

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
    chain: ChainSpec,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
    metrics: Option<SocketAddr>,

    #[clap(flatten)]
    network: NetworkArgs,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug")]
    tip: Option<H256>,

    /// Runs the sync only up to the specified block
    #[arg(long = "debug.max-block", help_heading = "Debug")]
    max_block: Option<u64>,

    #[clap(flatten)]
    rpc: RpcServerArgs,
}

impl Command {
    /// Execute `node` command
    // TODO: RPC
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", crate_version!());

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        let mut config: Config = self.load_config()?;
        info!(target: "reth::cli", path = %self.db, "Configuration loaded");

        info!(target: "reth::cli", path = %self.db, "Opening database");
        let db = Arc::new(init_db(&self.db)?);
        info!(target: "reth::cli", "Database opened");

        self.start_metrics_endpoint()?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        init_genesis(db.clone(), self.chain.clone())?;

        let consensus = self.init_consensus()?;
        info!(target: "reth::cli", "Consensus engine initialized");

        self.init_trusted_nodes(&mut config);

        info!(target: "reth::cli", "Connecting to P2P network");
        let network_config =
            self.load_network_config(&config, Arc::clone(&db), ctx.task_executor.clone());
        let network = self.start_network(network_config, &ctx.task_executor, ()).await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");

        // TODO: Use the resolved secret to spawn the Engine API server
        // Look at `reth_rpc::AuthLayer` for integration hints
        let _secret = self.rpc.jwt_secret();

        // TODO(mattsse): cleanup, add cli args
        let _rpc_server = reth_rpc_builder::launch(
            ShareableDatabase::new(db.clone()),
            reth_transaction_pool::test_utils::testing_pool(),
            network.clone(),
            TransportRpcModuleConfig::default()
                .with_http(vec![RethRpcModule::Admin, RethRpcModule::Eth]),
            RpcServerConfig::default().with_http(Default::default()),
        )
        .await?;
        info!(target: "reth::cli", "Started RPC server");

        let (mut pipeline, events) = self
            .build_networked_pipeline(
                &mut config,
                network.clone(),
                &consensus,
                db.clone(),
                &ctx.task_executor,
            )
            .await?;

        ctx.task_executor.spawn(handle_events(events));

        // Run pipeline
        let (rx, tx) = tokio::sync::oneshot::channel();
        info!(target: "reth::cli", "Starting sync pipeline");
        ctx.task_executor.spawn_critical_blocking("pipeline task", async move {
            let res = pipeline.run(db.clone()).await;
            let _ = rx.send(res);
        });

        tx.await??;

        info!(target: "reth::cli", "Finishing up");
        Ok(())
    }

    async fn build_networked_pipeline(
        &self,
        config: &mut Config,
        network: NetworkHandle,
        consensus: &Arc<dyn Consensus>,
        db: Arc<Env<WriteMap>>,
        task_executor: &TaskExecutor,
    ) -> eyre::Result<(Pipeline<Env<WriteMap>, impl SyncStateUpdater>, impl Stream<Item = NodeEvent>)>
    {
        // building network downloaders using the fetch client
        let fetch_client = Arc::new(network.fetch_client().await?);

        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(fetch_client.clone(), consensus.clone())
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(fetch_client.clone(), consensus.clone(), db.clone())
            .into_task_with(task_executor);

        let mut pipeline = self
            .build_pipeline(config, header_downloader, body_downloader, network.clone(), consensus)
            .await?;

        let events = stream_select(
            network.event_listener().map(Into::into),
            pipeline.events().map(Into::into),
        );
        Ok((pipeline, events))
    }

    fn load_config(&self) -> eyre::Result<Config> {
        confy::load_path::<Config>(&self.config).wrap_err("Could not load config")
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

    fn start_metrics_endpoint(&self) -> eyre::Result<()> {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::initialize(listen_addr)
        } else {
            Ok(())
        }
    }

    fn init_consensus(&self) -> eyre::Result<Arc<dyn Consensus>> {
        let (consensus, notifier) = BeaconConsensus::builder().build(self.chain.clone());

        if let Some(tip) = self.tip {
            debug!(target: "reth::cli", %tip, "Tip manually set");
            notifier.send(ForkchoiceState {
                head_block_hash: tip,
                safe_block_hash: tip,
                finalized_block_hash: tip,
            })?;
        } else {
            let warn_msg = "No tip specified. \
            reth cannot communicate with consensus clients, \
            so a tip must manually be provided for the online stages with --debug.tip <HASH>.";
            warn!(target: "reth::cli", warn_msg);
        }

        Ok(consensus)
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    async fn start_network<C>(
        &self,
        config: NetworkConfig<C>,
        task_executor: &TaskExecutor,
        // TODO: integrate pool
        _pool: (),
    ) -> Result<NetworkHandle, NetworkError>
    where
        C: BlockProvider + HeaderProvider + 'static,
    {
        let client = config.client.clone();
        let (handle, network, _txpool, eth) =
            NetworkManager::builder(config).await?.request_handler(client).split_with_handle();

        let known_peers_file = self.network.persistent_peers_file();
        task_executor.spawn_critical_with_signal("p2p network task", |shutdown| async move {
            run_network_until_shutdown(shutdown, network, known_peers_file).await
        });

        task_executor.spawn_critical("p2p eth request handler", async move { eth.await });

        // TODO spawn pool

        Ok(handle)
    }

    fn fetch_head(&self, db: Arc<Env<WriteMap>>) -> Result<Head, reth_interfaces::db::Error> {
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

    fn load_network_config(
        &self,
        config: &Config,
        db: Arc<Env<WriteMap>>,
        executor: TaskExecutor,
    ) -> NetworkConfig<ShareableDatabase<Arc<Env<WriteMap>>>> {
        let head = self.fetch_head(Arc::clone(&db)).expect("the head block is missing");

        self.network
            .network_config(config, self.chain.clone())
            .with_task_executor(Box::new(executor))
            .set_head(head)
            .build(Arc::new(ShareableDatabase::new(db)))
    }

    async fn build_pipeline<H, B, U>(
        &self,
        config: &Config,
        header_downloader: H,
        body_downloader: B,
        updater: U,
        consensus: &Arc<dyn Consensus>,
    ) -> eyre::Result<Pipeline<Env<WriteMap>, U>>
    where
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
        U: SyncStateUpdater + StatusUpdater + Clone + 'static,
    {
        let stage_conf = &config.stages;

        let mut builder = Pipeline::builder();

        if let Some(max_block) = self.max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let pipeline = builder
            .with_sync_state_updater(updater.clone())
            .add_stages(
                DefaultStages::new(consensus.clone(), header_downloader, body_downloader, updater)
                    .set(TotalDifficultyStage {
                        chain_spec: self.chain.clone(),
                        commit_threshold: stage_conf.total_difficulty.commit_threshold,
                    })
                    .set(SenderRecoveryStage {
                        commit_threshold: stage_conf.sender_recovery.commit_threshold,
                    })
                    .set(ExecutionStage {
                        chain_spec: self.chain.clone(),
                        commit_threshold: stage_conf.execution.commit_threshold,
                    }),
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
    C: BlockProvider + HeaderProvider + 'static,
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
            match std::fs::write(&file_path, known_peers) {
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

/// The current high-level state of the node.
#[derive(Default)]
struct NodeState {
    /// The number of connected peers.
    connected_peers: usize,
    /// The stage currently being executed.
    current_stage: Option<StageId>,
    /// The current checkpoint of the executing stage.
    current_checkpoint: BlockNumber,
}

impl NodeState {
    async fn handle_pipeline_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Running { stage_id, stage_progress } => {
                let notable = self.current_stage.is_none();
                self.current_stage = Some(stage_id);
                self.current_checkpoint = stage_progress.unwrap_or_default();

                if notable {
                    info!(target: "reth::cli", stage = %stage_id, from = stage_progress, "Executing stage");
                }
            }
            PipelineEvent::Ran { stage_id, result } => {
                let notable = result.stage_progress > self.current_checkpoint;
                self.current_checkpoint = result.stage_progress;
                if result.done {
                    self.current_stage = None;
                    info!(target: "reth::cli", stage = %stage_id, checkpoint = result.stage_progress, "Stage finished executing");
                } else if notable {
                    info!(target: "reth::cli", stage = %stage_id, checkpoint = result.stage_progress, "Stage committed progress");
                }
            }
            _ => (),
        }
    }

    async fn handle_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::SessionEstablished { peer_id, status, .. } => {
                self.connected_peers += 1;
                info!(target: "reth::cli", connected_peers = self.connected_peers, peer_id = %peer_id, best_block = %status.blockhash, "Peer connected");
            }
            NetworkEvent::SessionClosed { peer_id, reason } => {
                self.connected_peers -= 1;
                let reason = reason.map(|s| s.to_string()).unwrap_or_else(|| "None".to_string());
                warn!(target: "reth::cli", connected_peers = self.connected_peers, peer_id = %peer_id, %reason, "Peer disconnected.");
            }
            _ => (),
        }
    }
}

/// A node event.
pub enum NodeEvent {
    /// A network event.
    Network(NetworkEvent),
    /// A sync pipeline event.
    Pipeline(PipelineEvent),
}

impl From<NetworkEvent> for NodeEvent {
    fn from(evt: NetworkEvent) -> NodeEvent {
        NodeEvent::Network(evt)
    }
}

impl From<PipelineEvent> for NodeEvent {
    fn from(evt: PipelineEvent) -> NodeEvent {
        NodeEvent::Pipeline(evt)
    }
}

/// Displays relevant information to the user from components of the node, and periodically
/// displays the high-level status of the node.
pub async fn handle_events(mut events: impl Stream<Item = NodeEvent> + Unpin) {
    let mut state = NodeState::default();

    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            Some(event) = events.next() => {
                match event {
                    NodeEvent::Network(event) => {
                        state.handle_network_event(event).await;
                    },
                    NodeEvent::Pipeline(event) => {
                        state.handle_pipeline_event(event).await;
                    }
                }
            },
            _ = interval.tick() => {
                let stage = state.current_stage.map(|id| id.to_string()).unwrap_or_else(|| "None".to_string());
                info!(target: "reth::cli", connected_peers = state.connected_peers, %stage, checkpoint = state.current_checkpoint, "Status");
            }
        }
    }
}
