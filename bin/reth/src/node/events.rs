//! Support for handling events emitted by node components.

use crate::node::cl_events::ConsensusLayerHealthEvent;
use futures::Stream;
use reth_beacon_consensus::BeaconConsensusEngineEvent;
use reth_interfaces::consensus::ForkchoiceState;
use reth_network::{NetworkEvent, NetworkHandle};
use reth_network_api::PeersInfo;
use reth_primitives::{
    stage::{EntitiesCheckpoint, StageCheckpoint, StageId},
    BlockNumber,
};
use reth_stages::{ExecOutput, PipelineEvent};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::time::Interval;
use tracing::{info, warn};

/// Interval of reporting node state.
const INFO_MESSAGE_INTERVAL: Duration = Duration::from_secs(30);

/// The current high-level state of the node.
struct NodeState {
    /// Connection to the network.
    network: Option<NetworkHandle>,
    /// The stage currently being executed.
    current_stage: Option<StageId>,
    /// The ETA for the current stage.
    eta: Eta,
    /// The current checkpoint of the executing stage.
    current_checkpoint: StageCheckpoint,
    /// The latest canonical block added in the consensus engine.
    latest_canonical_engine_block: Option<BlockNumber>,
}

impl NodeState {
    fn new(network: Option<NetworkHandle>, latest_block_number: Option<BlockNumber>) -> Self {
        Self {
            network,
            current_stage: None,
            eta: Eta::default(),
            current_checkpoint: StageCheckpoint::new(0),
            latest_canonical_engine_block: latest_block_number,
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
                        pipeline_stages = %format!("{pipeline_position}/{pipeline_total}"),
                        stage = %stage_id,
                        from = self.current_checkpoint.block_number,
                        checkpoint = %self.current_checkpoint,
                        eta = %self.eta.fmt_for_stage(stage_id),
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
                self.eta.update(self.current_checkpoint);

                info!(
                    pipeline_stages = %format!("{pipeline_position}/{pipeline_total}"),
                    stage = %stage_id,
                    block = checkpoint.block_number,
                    %checkpoint,
                    eta = %self.eta.fmt_for_stage(stage_id),
                    "{}",
                    if done {
                        "Stage finished executing"
                    } else {
                        "Stage committed progress"
                    }
                );

                if done {
                    self.current_stage = None;
                    self.eta = Eta::default();
                }
            }
            _ => (),
        }
    }

    fn handle_network_event(&mut self, _: NetworkEvent) {
        // NOTE(onbjerg): This used to log established/disconnecting sessions, but this is already
        // logged in the networking component. I kept this stub in case we want to catch other
        // networking events later on.
    }

    fn handle_consensus_engine_event(&mut self, event: BeaconConsensusEngineEvent) {
        match event {
            BeaconConsensusEngineEvent::ForkchoiceUpdated(state, status) => {
                let ForkchoiceState { head_block_hash, safe_block_hash, finalized_block_hash } =
                    state;
                info!(
                    ?head_block_hash,
                    ?safe_block_hash,
                    ?finalized_block_hash,
                    ?status,
                    "Forkchoice updated"
                );
            }
            BeaconConsensusEngineEvent::CanonicalBlockAdded(block) => {
                self.latest_canonical_engine_block = Some(block.number);

                info!(number=block.number, hash=?block.hash, "Block added to canonical chain");
            }
            BeaconConsensusEngineEvent::ForkBlockAdded(block) => {
                info!(number=block.number, hash=?block.hash, "Block added to fork chain");
            }
        }
    }

    fn handle_consensus_layer_health_event(&self, event: ConsensusLayerHealthEvent) {
        match event {
            ConsensusLayerHealthEvent::NeverSeen => {
                warn!("Post-merge network, but never seen beacon client. Please launch one to follow the chain!")
            }
            ConsensusLayerHealthEvent::HasNotBeenSeenForAWhile(period) => {
                warn!(?period, "Post-merge network, but no beacon client seen for a while. Please launch one to follow the chain!")
            }
            ConsensusLayerHealthEvent::NeverReceivedUpdates => {
                warn!("Beacon client online, but never received consensus updates. Please ensure your beacon client is operational to follow the chain!")
            }
            ConsensusLayerHealthEvent::HaveNotReceivedUpdatesForAWhile(period) => {
                warn!(?period, "Beacon client online, but no consensus updates received for a while. Please fix your beacon client to follow the chain!")
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
    /// A Consensus Layer health event.
    ConsensusLayerHealth(ConsensusLayerHealthEvent),
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

impl From<ConsensusLayerHealthEvent> for NodeEvent {
    fn from(event: ConsensusLayerHealthEvent) -> Self {
        NodeEvent::ConsensusLayerHealth(event)
    }
}

/// Displays relevant information to the user from components of the node, and periodically
/// displays the high-level status of the node.
pub async fn handle_events<E>(
    network: Option<NetworkHandle>,
    latest_block_number: Option<BlockNumber>,
    events: E,
) where
    E: Stream<Item = NodeEvent> + Unpin,
{
    let state = NodeState::new(network, latest_block_number);

    let mut info_interval = tokio::time::interval(INFO_MESSAGE_INTERVAL);
    info_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let handler = EventHandler { state, events, info_interval };
    handler.await
}

/// Handles events emitted by the node and logs them accordingly.
#[pin_project::pin_project]
struct EventHandler<E> {
    state: NodeState,
    #[pin]
    events: E,
    #[pin]
    info_interval: Interval,
}

impl<E> Future for EventHandler<E>
where
    E: Stream<Item = NodeEvent> + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        while this.info_interval.poll_tick(cx).is_ready() {
            if let Some(stage_id) = this.state.current_stage {
                info!(
                    target: "reth::cli",
                    connected_peers = this.state.num_connected_peers(),
                    stage = %stage_id.to_string(),
                    checkpoint = %this.state.current_checkpoint,
                    eta = %this.state.eta.fmt_for_stage(stage_id),
                    "Status"
                );
            } else {
                info!(
                    target: "reth::cli",
                    connected_peers = this.state.num_connected_peers(),
                    latest_block = this.state.latest_canonical_engine_block.unwrap_or(this.state.current_checkpoint.block_number),
                    "Status"
                );
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
                NodeEvent::ConsensusLayerHealth(event) => {
                    this.state.handle_consensus_layer_health_event(event)
                }
            }
        }

        Poll::Pending
    }
}

/// A container calculating the estimated time that a stage will complete in, based on stage
/// checkpoints reported by the pipeline.
///
/// One `Eta` is only valid for a single stage.
#[derive(Default)]
struct Eta {
    /// The last stage checkpoint
    last_checkpoint: EntitiesCheckpoint,
    /// The last time the stage reported its checkpoint
    last_checkpoint_time: Option<Instant>,
    /// The current ETA
    eta: Option<Duration>,
}

impl Eta {
    /// Update the ETA given the checkpoint, if possible.
    fn update(&mut self, checkpoint: StageCheckpoint) {
        let Some(current) = checkpoint.entities() else { return };

        if let Some(last_checkpoint_time) = &self.last_checkpoint_time {
            let processed_since_last = current.processed - self.last_checkpoint.processed;
            let elapsed = last_checkpoint_time.elapsed();
            let per_second = processed_since_last as f64 / elapsed.as_secs_f64();

            self.eta = Duration::try_from_secs_f64(
                ((current.total - current.processed) as f64) / per_second,
            )
            .ok();
        }

        self.last_checkpoint = current;
        self.last_checkpoint_time = Some(Instant::now());
    }

    /// Format ETA for a given stage.
    ///
    /// NOTE: Currently ETA is disabled for Headers and Bodies stages until we find better
    /// heuristics for calculation.
    fn fmt_for_stage(&self, stage: StageId) -> String {
        if matches!(stage, StageId::Headers | StageId::Bodies) {
            String::from("unknown")
        } else {
            format!("{}", self)
        }
    }
}

impl std::fmt::Display for Eta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some((eta, last_checkpoint_time)) = self.eta.zip(self.last_checkpoint_time) {
            let remaining = eta.checked_sub(last_checkpoint_time.elapsed());

            if let Some(remaining) = remaining {
                return write!(
                    f,
                    "{}",
                    humantime::format_duration(Duration::from_secs(remaining.as_secs()))
                )
            }
        }

        write!(f, "unknown")
    }
}

#[cfg(test)]
mod tests {
    use crate::node::events::Eta;
    use std::time::{Duration, Instant};

    #[test]
    fn eta_display_no_milliseconds() {
        let eta = Eta {
            last_checkpoint_time: Some(Instant::now()),
            eta: Some(Duration::from_millis(
                13 * 60 * 1000 + // Minutes
                    37 * 1000 + // Seconds
                    999, // Milliseconds
            )),
            ..Default::default()
        }
        .to_string();

        assert_eq!(eta, "13m 37s");
    }
}
