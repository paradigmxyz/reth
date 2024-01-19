//! Support for handling events emitted by node components.

use crate::commands::node::cl_events::ConsensusLayerHealthEvent;
use futures::Stream;
use reth_beacon_consensus::BeaconConsensusEngineEvent;
use reth_db::{database::Database, database_metrics::DatabaseMetadata};
use reth_interfaces::consensus::ForkchoiceState;
use reth_network::{NetworkEvent, NetworkHandle};
use reth_network_api::PeersInfo;
use reth_primitives::{
    stage::{EntitiesCheckpoint, StageCheckpoint, StageId},
    BlockNumber,
};
use reth_prune::PrunerEvent;
use reth_stages::{ExecOutput, PipelineEvent};
use std::{
    fmt::{Display, Formatter},
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::time::Interval;
use tracing::{info, warn};

/// Interval of reporting node state.
const INFO_MESSAGE_INTERVAL: Duration = Duration::from_secs(25);

/// The current high-level state of the node.
struct NodeState<DB> {
    /// Database environment.
    /// Used for freelist calculation reported in the "Status" log message.
    /// See [EventHandler::poll].
    db: DB,
    /// Connection to the network.
    network: Option<NetworkHandle>,
    /// The stage currently being executed.
    current_stage: Option<CurrentStage>,
    /// The latest block reached by either pipeline or consensus engine.
    latest_block: Option<BlockNumber>,
}

impl<DB> NodeState<DB> {
    fn new(db: DB, network: Option<NetworkHandle>, latest_block: Option<BlockNumber>) -> Self {
        Self { db, network, current_stage: None, latest_block }
    }

    fn num_connected_peers(&self) -> usize {
        self.network.as_ref().map(|net| net.num_connected_peers()).unwrap_or_default()
    }

    /// Processes an event emitted by the pipeline
    fn handle_pipeline_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Run { pipeline_stages_progress, stage_id, checkpoint, target } => {
                let checkpoint = checkpoint.unwrap_or_default();
                let current_stage = CurrentStage {
                    stage_id,
                    eta: match &self.current_stage {
                        Some(current_stage) if current_stage.stage_id == stage_id => {
                            current_stage.eta
                        }
                        _ => Eta::default(),
                    },
                    checkpoint,
                    target,
                };

                let stage_progress = OptionalField(
                    checkpoint.entities().and_then(|entities| entities.fmt_percentage()),
                );

                if let Some(stage_eta) = current_stage.eta.fmt_for_stage(stage_id) {
                    info!(
                        pipeline_stages = %pipeline_stages_progress,
                        stage = %stage_id,
                        checkpoint = %checkpoint.block_number,
                        target = %OptionalField(target),
                        %stage_progress,
                        %stage_eta,
                        "Executing stage",
                    );
                } else {
                    info!(
                        pipeline_stages = %pipeline_stages_progress,
                        stage = %stage_id,
                        checkpoint = %checkpoint.block_number,
                        target = %OptionalField(target),
                        %stage_progress,
                        "Executing stage",
                    );
                }

                self.current_stage = Some(current_stage);
            }
            PipelineEvent::Ran {
                pipeline_stages_progress,
                stage_id,
                result: ExecOutput { checkpoint, done },
            } => {
                if stage_id.is_finish() {
                    self.latest_block = Some(checkpoint.block_number);
                }

                if let Some(current_stage) = self.current_stage.as_mut() {
                    current_stage.checkpoint = checkpoint;
                    current_stage.eta.update(checkpoint);

                    let target = OptionalField(current_stage.target);
                    let stage_progress = OptionalField(
                        checkpoint.entities().and_then(|entities| entities.fmt_percentage()),
                    );

                    let message =
                        if done { "Stage finished executing" } else { "Stage committed progress" };

                    if let Some(stage_eta) = current_stage.eta.fmt_for_stage(stage_id) {
                        info!(
                            pipeline_stages = %pipeline_stages_progress,
                            stage = %stage_id,
                            checkpoint = %checkpoint.block_number,
                            %target,
                            %stage_progress,
                            %stage_eta,
                            "{message}",
                        )
                    } else {
                        info!(
                            pipeline_stages = %pipeline_stages_progress,
                            stage = %stage_id,
                            checkpoint = %checkpoint.block_number,
                            %target,
                            %stage_progress,
                            "{message}",
                        )
                    }
                }

                if done {
                    self.current_stage = None;
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
                info!(number=block.number, hash=?block.hash, "Block added to canonical chain");
            }
            BeaconConsensusEngineEvent::CanonicalChainCommitted(head, elapsed) => {
                self.latest_block = Some(head.number);

                info!(number=head.number, hash=?head.hash, ?elapsed, "Canonical chain committed");
            }
            BeaconConsensusEngineEvent::ForkBlockAdded(block) => {
                info!(number=block.number, hash=?block.hash, "Block added to fork chain");
            }
        }
    }

    fn handle_consensus_layer_health_event(&self, event: ConsensusLayerHealthEvent) {
        // If pipeline is running, it's fine to not receive any messages from the CL.
        // So we need to report about CL health only when pipeline is idle.
        if self.current_stage.is_none() {
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

    fn handle_pruner_event(&self, event: PrunerEvent) {
        match event {
            PrunerEvent::Finished { tip_block_number, elapsed, stats } => {
                info!(tip_block_number, ?elapsed, ?stats, "Pruner finished");
            }
        }
    }
}

impl<DB: DatabaseMetadata> NodeState<DB> {
    fn freelist(&self) -> Option<usize> {
        self.db.metadata().freelist_size()
    }
}

/// Helper type for formatting of optional fields:
/// - If [Some(x)], then `x` is written
/// - If [None], then `None` is written
struct OptionalField<T: Display>(Option<T>);

impl<T: Display> Display for OptionalField<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(field) = &self.0 {
            write!(f, "{field}")
        } else {
            write!(f, "None")
        }
    }
}

/// The stage currently being executed.
struct CurrentStage {
    stage_id: StageId,
    eta: Eta,
    checkpoint: StageCheckpoint,
    target: Option<BlockNumber>,
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
    /// A pruner event
    Pruner(PrunerEvent),
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

impl From<PrunerEvent> for NodeEvent {
    fn from(event: PrunerEvent) -> Self {
        NodeEvent::Pruner(event)
    }
}

/// Displays relevant information to the user from components of the node, and periodically
/// displays the high-level status of the node.
pub async fn handle_events<E, DB>(
    network: Option<NetworkHandle>,
    latest_block_number: Option<BlockNumber>,
    events: E,
    db: DB,
) where
    E: Stream<Item = NodeEvent> + Unpin,
    DB: DatabaseMetadata + Database + 'static,
{
    let state = NodeState::new(db, network, latest_block_number);

    let start = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut info_interval = tokio::time::interval_at(start, INFO_MESSAGE_INTERVAL);
    info_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let handler = EventHandler { state, events, info_interval };
    handler.await
}

/// Handles events emitted by the node and logs them accordingly.
#[pin_project::pin_project]
struct EventHandler<E, DB> {
    state: NodeState<DB>,
    #[pin]
    events: E,
    #[pin]
    info_interval: Interval,
}

impl<E, DB> Future for EventHandler<E, DB>
where
    E: Stream<Item = NodeEvent> + Unpin,
    DB: DatabaseMetadata + Database + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        while this.info_interval.poll_tick(cx).is_ready() {
            let freelist = OptionalField(this.state.freelist());

            if let Some(CurrentStage { stage_id, eta, checkpoint, target }) =
                &this.state.current_stage
            {
                let stage_progress = OptionalField(
                    checkpoint.entities().and_then(|entities| entities.fmt_percentage()),
                );

                if let Some(stage_eta) = eta.fmt_for_stage(*stage_id) {
                    info!(
                        target: "reth::cli",
                        connected_peers = this.state.num_connected_peers(),
                        %freelist,
                        stage = %stage_id,
                        checkpoint = checkpoint.block_number,
                        target = %OptionalField(*target),
                        %stage_progress,
                        %stage_eta,
                        "Status"
                    );
                } else {
                    info!(
                        target: "reth::cli",
                        connected_peers = this.state.num_connected_peers(),
                        %freelist,
                        stage = %stage_id,
                        checkpoint = checkpoint.block_number,
                        target = %OptionalField(*target),
                        %stage_progress,
                        "Status"
                    );
                }
            } else if let Some(latest_block) = this.state.latest_block {
                info!(
                    target: "reth::cli",
                    connected_peers = this.state.num_connected_peers(),
                    %freelist,
                    %latest_block,
                    "Status"
                );
            } else {
                info!(
                    target: "reth::cli",
                    connected_peers = this.state.num_connected_peers(),
                    %freelist,
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
                NodeEvent::Pruner(event) => {
                    this.state.handle_pruner_event(event);
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
#[derive(Default, Copy, Clone)]
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
    /// NOTE: Currently ETA is enabled only for the stages that have predictable progress.
    /// It's not the case for network-dependent ([StageId::Headers] and [StageId::Bodies]) and
    /// [StageId::Execution] stages.
    fn fmt_for_stage(&self, stage: StageId) -> Option<String> {
        if matches!(stage, StageId::Headers | StageId::Bodies | StageId::Execution) {
            None
        } else {
            Some(self.to_string())
        }
    }
}

impl Display for Eta {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
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
    use crate::commands::node::events::Eta;
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
