//! Main node command
//!
//! Starts the client
use crate::{
    dirs::{ConfigPath, DbPath, PlatformPath},
    prometheus_exporter,
    utils::{chainspec::genesis_value_parser, init::init_db, parse_socket_address},
    NetworkOpts, RpcServerOpts,
};
use clap::{crate_version, Parser};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{stream::select as stream_select, Stream, StreamExt};
use reth_consensus::beacon::BeaconConsensus;
use reth_db::mdbx::{Env, WriteMap};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    p2p::{bodies::downloader::BodyDownloader, headers::downloader::HeaderDownloader},
    sync::SyncStateUpdater,
};
use reth_net_nat::NatResolver;
use reth_network::{NetworkConfig, NetworkEvent, NetworkHandle};
use reth_network_api::NetworkInfo;
use reth_primitives::{BlockNumber, ChainSpec, H256};
use reth_provider::ShareableDatabase;
use reth_rpc_builder::{RethRpcModule, RpcServerConfig, TransportRpcModuleConfig};
use reth_staged_sync::{utils::init::init_genesis, Config};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, SenderRecoveryStage, TotalDifficultyStage},
};
use std::{io, net::SocketAddr, path::Path, sync::Arc, time::Duration};
use tracing::{debug, info, warn};

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
    network: NetworkOpts,

    #[arg(long, default_value = "any")]
    nat: NatResolver,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug")]
    tip: Option<H256>,

    /// Runs the sync only up to the specified block
    #[arg(long = "debug.max-block", help_heading = "Debug")]
    max_block: Option<u64>,

    #[clap(flatten)]
    rpc: RpcServerOpts,
}

impl Command {
    /// Execute `node` command
    // TODO: RPC
    pub async fn execute(self) -> eyre::Result<()> {
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

        debug!(target: "reth::cli", chainspec=?self.chain, "Initializing genesis");
        init_genesis(db.clone(), self.chain.clone())?;

        let consensus = self.init_consensus()?;
        info!(target: "reth::cli", "Consensus engine initialized");

        self.init_trusted_nodes(&mut config);

        info!(target: "reth::cli", "Connecting to P2P network");
        let netconf = self.load_network_config(&config, &db);
        let network = netconf.start_network().await?;

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");

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
            .build_networked_pipeline(&mut config, network.clone(), &consensus, db.clone())
            .await?;

        tokio::spawn(handle_events(events));

        // Run pipeline
        info!(target: "reth::cli", "Starting sync pipeline");
        pipeline.run(db.clone()).await?;

        // TODO: this is where we'd handle graceful shutdown by listening to ctrl-c

        if !self.network.no_persist_peers {
            dump_peers(self.network.peers_file.as_ref(), network).await?;
        }

        info!(target: "reth::cli", "Finishing up");
        Ok(())
    }

    async fn build_networked_pipeline(
        &self,
        config: &mut Config,
        network: NetworkHandle,
        consensus: &Arc<dyn Consensus>,
        db: Arc<Env<WriteMap>>,
    ) -> eyre::Result<(Pipeline<Env<WriteMap>, impl SyncStateUpdater>, impl Stream<Item = NodeEvent>)>
    {
        // building network downloaders using the fetch client
        let fetch_client = Arc::new(network.fetch_client().await?);

        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(fetch_client.clone(), consensus.clone())
            .as_task();

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(fetch_client.clone(), consensus.clone(), db.clone())
            .as_task();

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

    fn load_network_config(
        &self,
        config: &Config,
        db: &Arc<Env<WriteMap>>,
    ) -> NetworkConfig<ShareableDatabase<Arc<Env<WriteMap>>>> {
        let peers_file = (!self.network.no_persist_peers).then_some(&self.network.peers_file);
        config.network_config(
            db.clone(),
            self.chain.clone(),
            self.network.disable_discovery,
            self.network.bootnodes.clone(),
            self.nat,
            peers_file.map(|f| f.as_ref().to_path_buf()),
        )
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
        U: SyncStateUpdater,
    {
        let stage_conf = &config.stages;

        let mut builder = Pipeline::builder();

        if let Some(max_block) = self.max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let pipeline = builder
            .with_sync_state_updater(updater)
            .add_stages(
                OnlineStages::new(consensus.clone(), header_downloader, body_downloader).set(
                    TotalDifficultyStage {
                        chain_spec: self.chain.clone(),
                        commit_threshold: stage_conf.total_difficulty.commit_threshold,
                    },
                ),
            )
            .add_stages(
                OfflineStages::default()
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

/// Dumps peers to `file_path` for persistence.
async fn dump_peers(file_path: &Path, network: NetworkHandle) -> Result<(), io::Error> {
    info!(target : "net::peers", file = %file_path.display(), "Saving current peers");
    let known_peers = network.peers_handle().all_peers().await;

    tokio::fs::write(file_path, serde_json::to_string_pretty(&known_peers)?).await?;
    Ok(())
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
