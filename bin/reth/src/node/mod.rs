//! Main node command
//!
//! Starts the client
use crate::{
    config::Config,
    dirs::{ConfigPath, DbPath, PlatformPath},
    prometheus_exporter,
    utils::{
        chainspec::{chain_spec_value_parser, ChainSpecification},
        init::{init_db, init_genesis},
        parse_socket_address,
    },
    NetworkOpts,
};
use clap::{crate_version, Parser};
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{stream::select as stream_select, Stream, StreamExt};
use reth_consensus::BeaconConsensus;
use reth_downloaders::{bodies, headers};
use reth_executor::Config as ExecutorConfig;
use reth_interfaces::consensus::ForkchoiceState;
use reth_network::NetworkEvent;
use reth_primitives::{BlockNumber, H256};
use reth_stages::{
    metrics::HeaderMetrics,
    stages::{
        bodies::BodyStage, execution::ExecutionStage, headers::HeaderStage,
        sender_recovery::SenderRecoveryStage, total_difficulty::TotalDifficultyStage,
    },
    PipelineEvent, StageId,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::select;
use tokio_stream::wrappers::ReceiverStream;
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
        value_parser = chain_spec_value_parser
    )]
    chain: ChainSpecification,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
    metrics: Option<SocketAddr>,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug")]
    tip: Option<H256>,

    #[clap(flatten)]
    network: NetworkOpts,
}

impl Command {
    /// Execute `node` command
    // TODO: RPC
    pub async fn execute(&self) -> eyre::Result<()> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        let mut config: Config =
            confy::load_path(&self.config).wrap_err("Could not load config")?;
        config.peers.connect_trusted_nodes_only = self.network.trusted_only;
        if !self.network.trusted_peers.is_empty() {
            self.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        info!(target: "reth::cli", "reth {} starting", crate_version!());

        info!(target: "reth::cli", path = %self.db, "Opening database");
        let db = Arc::new(init_db(&self.db)?);
        info!(target: "reth::cli", "Database opened");

        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::initialize(listen_addr)?;
            HeaderMetrics::describe();
        }

        let chain_id = self.chain.consensus.chain_id;
        let consensus = Arc::new(BeaconConsensus::new(self.chain.consensus.clone()));
        let genesis_hash = init_genesis(db.clone(), self.chain.genesis.clone())?;

        let network = config
            .network_config(db.clone(), chain_id, genesis_hash, self.network.disable_discovery)
            .start_network()
            .await?;

        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        tokio::spawn(handle_events(stream_select(
            network.event_listener().map(Into::into),
            ReceiverStream::new(receiver).map(Into::into),
        )));

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");

        let fetch_client = Arc::new(network.fetch_client().await?);
        let mut pipeline = reth_stages::Pipeline::default()
            .with_sync_state_updater(network.clone())
            .with_channel(sender)
            .push(HeaderStage {
                downloader: headers::linear::LinearDownloadBuilder::default()
                    .batch_size(config.stages.headers.downloader_batch_size)
                    .retries(config.stages.headers.downloader_retries)
                    .build(consensus.clone(), fetch_client.clone()),
                consensus: consensus.clone(),
                client: fetch_client.clone(),
                network_handle: network.clone(),
                commit_threshold: config.stages.headers.commit_threshold,
                metrics: HeaderMetrics::default(),
            })
            .push(TotalDifficultyStage {
                commit_threshold: config.stages.total_difficulty.commit_threshold,
            })
            .push(BodyStage {
                downloader: Arc::new(
                    bodies::concurrent::ConcurrentDownloader::new(
                        fetch_client.clone(),
                        consensus.clone(),
                    )
                    .with_batch_size(config.stages.bodies.downloader_batch_size)
                    .with_retries(config.stages.bodies.downloader_retries)
                    .with_concurrency(config.stages.bodies.downloader_concurrency),
                ),
                consensus: consensus.clone(),
                commit_threshold: config.stages.bodies.commit_threshold,
            })
            .push(SenderRecoveryStage {
                batch_size: config.stages.sender_recovery.batch_size,
                commit_threshold: config.stages.sender_recovery.commit_threshold,
            })
            .push(ExecutionStage {
                config: ExecutorConfig::new_ethereum(),
                commit_threshold: config.stages.execution.commit_threshold,
            });

        if let Some(tip) = self.tip {
            debug!(target: "reth::cli", %tip, "Tip manually set");
            consensus.notify_fork_choice_state(ForkchoiceState {
                head_block_hash: tip,
                safe_block_hash: tip,
                finalized_block_hash: tip,
            })?;
        } else {
            warn!(target: "reth::cli", "No tip specified. reth cannot communicate with consensus clients, so a tip must manually be provided for the online stages with --debug.tip <HASH>.");
        }

        // Run pipeline
        info!(target: "reth::cli", "Starting sync pipeline");
        pipeline.run(db.clone()).await?;

        info!(target: "reth::cli", "Finishing up");
        Ok(())
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

/// A node event.
enum NodeEvent {
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
async fn handle_events(mut events: impl Stream<Item = NodeEvent> + Unpin) {
    let mut state = NodeState::default();

    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        select! {
            Some(event) = events.next() => {
                match event {
                    NodeEvent::Network(NetworkEvent::SessionEstablished { peer_id, status, .. }) => {
                        state.connected_peers += 1;
                        info!(target: "reth::cli", connected_peers = state.connected_peers, peer_id = %peer_id, best_block = %status.blockhash, "Peer connected");
                    },
                    NodeEvent::Network(NetworkEvent::SessionClosed { peer_id, reason }) => {
                        state.connected_peers -= 1;
                        let reason = reason.map(|s| s.to_string()).unwrap_or("None".to_string());
                        warn!(target: "reth::cli", connected_peers = state.connected_peers, peer_id = %peer_id, %reason, "Peer disconnected.");
                    },
                    NodeEvent::Pipeline(PipelineEvent::Running { stage_id, stage_progress }) => {
                        let notable = state.current_stage.is_none();
                        state.current_stage = Some(stage_id);
                        state.current_checkpoint = stage_progress.unwrap_or_default();

                        if notable {
                            info!(target: "reth::cli", stage = %stage_id, from = stage_progress, "Executing stage");
                        }
                    },
                    NodeEvent::Pipeline(PipelineEvent::Ran { stage_id, result }) => {
                        let notable = result.stage_progress > state.current_checkpoint;
                        state.current_checkpoint = result.stage_progress;
                        if result.done {
                            state.current_stage = None;
                            info!(target: "reth::cli", stage = %stage_id, checkpoint = result.stage_progress, "Stage finished executing");
                        } else if notable {
                            info!(target: "reth::cli", stage = %stage_id, checkpoint = result.stage_progress, "Stage committed progress");
                        }
                    }
                    _ => (),
                }
            },
            _ = interval.tick() => {
                let stage = state.current_stage.map(|id| id.to_string()).unwrap_or("None".to_string());
                info!(target: "reth::cli", connected_peers = state.connected_peers, %stage, checkpoint = state.current_checkpoint, "Status");
            }
        }
    }
}
