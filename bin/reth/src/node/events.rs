//! Support for handling events emitted by node components.

use futures::{Stream, StreamExt};
use reth_network::{NetworkEvent, NetworkHandle};
use reth_network_api::PeersInfo;
use reth_primitives::BlockNumber;
use reth_stages::{PipelineEvent, StageId};
use std::time::Duration;
use tracing::{info, warn};

/// The current high-level state of the node.
struct NodeState {
    /// Connection to the network
    network: Option<NetworkHandle>,
    /// The stage currently being executed.
    current_stage: Option<StageId>,
    /// The current checkpoint of the executing stage.
    current_checkpoint: BlockNumber,
}

impl NodeState {
    fn new(network: Option<NetworkHandle>) -> Self {
        Self { network, current_stage: None, current_checkpoint: 0 }
    }

    fn num_connected_peers(&self) -> usize {
        self.network.as_ref().map(|net| net.num_connected_peers()).unwrap_or_default()
    }

    /// Processes an event emitted by the pipeline
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
                info!(target: "reth::cli", connected_peers = self.num_connected_peers(), peer_id = %peer_id, best_block = %status.blockhash, "Peer connected");
            }
            NetworkEvent::SessionClosed { peer_id, reason } => {
                let reason = reason.map(|s| s.to_string()).unwrap_or_else(|| "None".to_string());
                warn!(target: "reth::cli", connected_peers = self.num_connected_peers(), peer_id = %peer_id, %reason, "Peer disconnected.");
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
pub async fn handle_events(
    network: Option<NetworkHandle>,
    mut events: impl Stream<Item = NodeEvent> + Unpin,
) {
    let mut state = NodeState::new(network);

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
                info!(target: "reth::cli", connected_peers = state.num_connected_peers(), %stage, checkpoint = state.current_checkpoint, "Status");
            }
        }
    }
}
