//! Support for handling events emitted by node components.

use futures::Stream;
use reth_beacon_consensus::BeaconConsensusEngineEvent;
use reth_network::{NetworkEvent, NetworkHandle};
use reth_network_api::PeersInfo;
use reth_primitives::stage::{StageCheckpoint, StageId};
use reth_provider::CanonChainTracker;
use reth_stages::{ExecOutput, PipelineEvent};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Interval;
use tracing::{debug, info, warn};

/// The current high-level state of the node.
struct NodeState<CC> {
    /// Connection to the network.
    network: Option<NetworkHandle>,
    /// The stage currently being executed.
    current_stage: Option<StageId>,
    /// The current checkpoint of the executing stage.
    current_checkpoint: StageCheckpoint,
    /// Canonical Chain tracker.
    canon_chain: CC,
}

impl<CC: CanonChainTracker> NodeState<CC> {
    fn new(network: Option<NetworkHandle>, canon_chain: CC) -> Self {
        Self {
            network,
            current_stage: None,
            current_checkpoint: StageCheckpoint::new(0),
            canon_chain,
        }
    }

    fn num_connected_peers(&self) -> usize {
        self.network.as_ref().map(|net| net.num_connected_peers()).unwrap_or_default()
    }

    /// Processes an event emitted by the pipeline
    fn handle_pipeline_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Running { pipeline_position, pipeline_total, stage_id, checkpoint } => {
                let notable = self.current_stage.is_none();
                self.current_stage = Some(stage_id);
                self.current_checkpoint = checkpoint.unwrap_or_default();

                if notable {
                    info!(
                        target: "reth::cli",
                        pipeline_stages = %format!("{pipeline_position}/{pipeline_total}"),
                        stage = %stage_id,
                        from = self.current_checkpoint.block_number,
                        checkpoint = %self.current_checkpoint,
                        "Executing stage",
                    );
                }
            }
            PipelineEvent::Ran {
                pipeline_position,
                pipeline_total,
                stage_id,
                result: ExecOutput { checkpoint, done },
            } => {
                self.current_checkpoint = checkpoint;

                if done {
                    self.current_stage = None;
                }

                info!(
                    target: "reth::cli",
                    pipeline_stages = %format!("{pipeline_position}/{pipeline_total}"),
                    stage = %stage_id,
                    progress = checkpoint.block_number,
                    %checkpoint,
                    "{}",
                    if done {
                        "Stage finished executing"
                    } else {
                        "Stage committed progress"
                    }
                );
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
            BeaconConsensusEngineEvent::CanonicalBlockAdded(block) => {
                info!(target: "reth::cli", number=block.number, hash=?block.hash, "Block added to canonical chain");
            }
            BeaconConsensusEngineEvent::ForkBlockAdded(block) => {
                info!(target: "reth::cli", number=block.number, hash=?block.hash, "Block added to fork chain");
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
pub async fn handle_events<CC, E>(network: Option<NetworkHandle>, canon_chain: CC, events: E)
where
    CC: CanonChainTracker,
    E: Stream<Item = NodeEvent> + Unpin,
{
    let state = NodeState::new(network, canon_chain);

    let mut info_interval = tokio::time::interval(Duration::from_secs(30));
    info_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut consensus_client_interval = tokio::time::interval(Duration::from_secs(5 * 60));
    consensus_client_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let handler = EventHandler { state, events, info_interval, consensus_client_interval };
    handler.await
}

/// Handles events emitted by the node and logs them accordingly.
#[pin_project::pin_project]
struct EventHandler<E, CC> {
    state: NodeState<CC>,
    #[pin]
    events: E,
    #[pin]
    info_interval: Interval,
    #[pin]
    consensus_client_interval: Interval,
}

impl<E, CC> Future for EventHandler<E, CC>
where
    E: Stream<Item = NodeEvent> + Unpin,
    CC: CanonChainTracker,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        if this.info_interval.poll_tick(cx).is_ready() {
            let stage = this
                .state
                .current_stage
                .map(|id| id.to_string())
                .unwrap_or_else(|| "None".to_string());
            info!(target: "reth::cli", connected_peers = this.state.num_connected_peers(), %stage, checkpoint = %this.state.current_checkpoint, "Status");
        }

        if this.consensus_client_interval.poll_tick(cx).is_ready() {
            if this.state.canon_chain.last_exchanged_transition_configuration_timestamp().is_some()
            {
                if let Some(last_received_update_timestamp) =
                    this.state.canon_chain.last_received_update_timestamp()
                {
                    if last_received_update_timestamp.elapsed() > Duration::from_secs(2 * 60) {
                        warn!(target: "reth::cli", "Beacon client online, but no consensus updates received in a while. Please fix your beacon client to follow the chain!")
                    }
                } else {
                    warn!(target: "reth::cli", "Beacon client online, but never received consensus updates. Please ensure your beacon client is operational to follow the chain!");
                }
            } else {
                warn!(target: "reth::cli", "Post-merge network, but no beacon client seen. Please launch one to follow the chain!")
            }
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
