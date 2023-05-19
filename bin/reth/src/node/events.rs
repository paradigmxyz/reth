//! Support for handling events emitted by node components.

use futures::Stream;
use reth_beacon_consensus::BeaconConsensusEngineEvent;
use reth_network::{NetworkEvent, NetworkHandle};
use reth_network_api::PeersInfo;
use reth_primitives::{BlockNumber, StageCheckpoint};
use reth_stages::{ExecOutput, PipelineEvent, StageId};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Interval;
use tracing::{debug, info};

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
    fn handle_pipeline_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Running { stage_id, stage_progress } => {
                let notable = self.current_stage.is_none();
                self.current_stage = Some(stage_id);
                self.current_checkpoint = stage_progress.unwrap_or_default();

                if notable {
                    info!(target: "reth::cli", stage = %stage_id, from = stage_progress, "Executing stage");
                }
            }
            PipelineEvent::Ran {
                stage_id,
                result: ExecOutput { checkpoint: StageCheckpoint { block_number, .. }, done },
            } => {
                let notable = block_number > self.current_checkpoint;
                self.current_checkpoint = block_number;
                if done {
                    self.current_stage = None;
                    info!(target: "reth::cli", stage = %stage_id, checkpoint = block_number, "Stage finished executing");
                } else if notable {
                    info!(target: "reth::cli", stage = %stage_id, checkpoint = block_number, "Stage committed progress");
                }
            }
            _ => (),
        }
    }

    fn handle_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::SessionEstablished { peer_id, status, .. } => {
                info!(target: "reth::cli", connected_peers = self.num_connected_peers(), peer_id = %peer_id, best_block = %status.blockhash, "Peer connected");
            }
            NetworkEvent::SessionClosed { peer_id, reason } => {
                let reason = reason.map(|s| s.to_string()).unwrap_or_else(|| "None".to_string());
                debug!(target: "reth::cli", connected_peers = self.num_connected_peers(), peer_id = %peer_id, %reason, "Peer disconnected.");
            }
            _ => (),
        }
    }

    fn handle_consensus_engine_event(&self, event: BeaconConsensusEngineEvent) {
        match event {
            BeaconConsensusEngineEvent::ForkchoiceUpdated(state) => {
                info!(target: "reth::cli", ?state, "Forkchoice updated");
            }
            BeaconConsensusEngineEvent::CanonicalBlockAdded(number, hash) => {
                info!(target: "reth::cli", number, ?hash, "Block added to canonical chain");
            }
            BeaconConsensusEngineEvent::ForkBlockAdded(number, hash) => {
                info!(target: "reth::cli", number, ?hash, "Block added to fork chain");
            }
        }
    }
}

/// A node event.
#[derive(Debug)]
pub enum NodeEvent {
    /// A network event.
    Network(NetworkEvent),
    /// A sync pipeline event.
    Pipeline(PipelineEvent),
    /// A consensus engine event.
    ConsensusEngine(BeaconConsensusEngineEvent),
}

impl From<NetworkEvent> for NodeEvent {
    fn from(event: NetworkEvent) -> NodeEvent {
        NodeEvent::Network(event)
    }
}

impl From<PipelineEvent> for NodeEvent {
    fn from(event: PipelineEvent) -> NodeEvent {
        NodeEvent::Pipeline(event)
    }
}

impl From<BeaconConsensusEngineEvent> for NodeEvent {
    fn from(event: BeaconConsensusEngineEvent) -> Self {
        NodeEvent::ConsensusEngine(event)
    }
}

/// Displays relevant information to the user from components of the node, and periodically
/// displays the high-level status of the node.
pub async fn handle_events(
    network: Option<NetworkHandle>,
    events: impl Stream<Item = NodeEvent> + Unpin,
) {
    let state = NodeState::new(network);

    let mut info_interval = tokio::time::interval(Duration::from_secs(30));
    info_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let handler = EventHandler { state, events, info_interval };
    handler.await
}

/// Handles events emitted by the node and logs them accordingly.
#[pin_project::pin_project]
struct EventHandler<St> {
    state: NodeState,
    #[pin]
    events: St,
    #[pin]
    info_interval: Interval,
}

impl<St> Future for EventHandler<St>
where
    St: Stream<Item = NodeEvent> + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        while this.info_interval.poll_tick(cx).is_ready() {
            let stage = this
                .state
                .current_stage
                .map(|id| id.to_string())
                .unwrap_or_else(|| "None".to_string());
            info!(target: "reth::cli", connected_peers = this.state.num_connected_peers(), %stage, checkpoint = this.state.current_checkpoint, "Status");
        }

        while let Poll::Ready(Some(event)) = this.events.as_mut().poll_next(cx) {
            match event {
                NodeEvent::Network(event) => {
                    this.state.handle_network_event(event);
                }
                NodeEvent::Pipeline(event) => {
                    this.state.handle_pipeline_event(event);
                }
                NodeEvent::ConsensusEngine(event) => {
                    this.state.handle_consensus_engine_event(event);
                }
            }
        }

        Poll::Pending
    }
}
