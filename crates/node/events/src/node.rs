//! Support for handling events emitted by node components.

use crate::cl::ConsensusLayerHealthEvent;
use alloy_primitives::{BlockNumber, B256};
use alloy_rpc_types_engine::ForkchoiceState;
use futures::Stream;
use reth_beacon_consensus::{
    BeaconConsensusEngineEvent, ConsensusEngineLiveSyncProgress, ForkchoiceStatus,
};
use reth_network::NetworkEvent;
use reth_network_api::PeersInfo;
use reth_primitives::constants;
use reth_primitives_traits::{format_gas, format_gas_throughput};
use reth_prune::PrunerEvent;
use reth_stages::{EntitiesCheckpoint, ExecOutput, PipelineEvent, StageCheckpoint, StageId};
use reth_static_file::StaticFileProducerEvent;
use std::{
    fmt::{Display, Formatter},
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::time::Interval;
use tracing::{debug, info, warn};

/// Interval of reporting node state.
const INFO_MESSAGE_INTERVAL: Duration = Duration::from_secs(25);

/// The current high-level state of the node, including the node's database environment, network
/// connections, current processing stage, and the latest block information. It provides
/// methods to handle different types of events that affect the node's state, such as pipeline
/// events, network events, and consensus engine events.
struct NodeState {
    /// Information about connected peers.
    peers_info: Option<Box<dyn PeersInfo>>,
    /// The stage currently being executed.
    current_stage: Option<CurrentStage>,
    /// The latest block reached by either pipeline or consensus engine.
    latest_block: Option<BlockNumber>,
    /// The time of the latest block seen by the pipeline
    latest_block_time: Option<u64>,
    /// Hash of the head block last set by fork choice update
    head_block_hash: Option<B256>,
    /// Hash of the safe block last set by fork choice update
    safe_block_hash: Option<B256>,
    /// Hash of finalized block last set by fork choice update
    finalized_block_hash: Option<B256>,
}

impl NodeState {
    const fn new(
        peers_info: Option<Box<dyn PeersInfo>>,
        latest_block: Option<BlockNumber>,
    ) -> Self {
        Self {
            peers_info,
            current_stage: None,
            latest_block,
            latest_block_time: None,
            head_block_hash: None,
            safe_block_hash: None,
            finalized_block_hash: None,
        }
    }

    fn num_connected_peers(&self) -> usize {
        self.peers_info.as_ref().map(|info| info.num_connected_peers()).unwrap_or_default()
    }

    fn build_current_stage(
        &self,
        stage_id: StageId,
        checkpoint: StageCheckpoint,
        target: Option<BlockNumber>,
    ) -> CurrentStage {
        let (eta, entities_checkpoint) = self
            .current_stage
            .as_ref()
            .filter(|current_stage| current_stage.stage_id == stage_id)
            .map_or_else(
                || (Eta::default(), None),
                |current_stage| (current_stage.eta, current_stage.entities_checkpoint),
            );

        CurrentStage { stage_id, eta, checkpoint, entities_checkpoint, target }
    }

    /// Processes an event emitted by the pipeline
    fn handle_pipeline_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Prepare { pipeline_stages_progress, stage_id, checkpoint, target } => {
                let checkpoint = checkpoint.unwrap_or_default();
                let current_stage = self.build_current_stage(stage_id, checkpoint, target);

                info!(
                    pipeline_stages = %pipeline_stages_progress,
                    stage = %stage_id,
                    checkpoint = %checkpoint.block_number,
                    target = %OptionalField(target),
                    "Preparing stage",
                );

                self.current_stage = Some(current_stage);
            }
            PipelineEvent::Run { pipeline_stages_progress, stage_id, checkpoint, target } => {
                let checkpoint = checkpoint.unwrap_or_default();
                let current_stage = self.build_current_stage(stage_id, checkpoint, target);

                if let Some(stage_eta) = current_stage.eta.fmt_for_stage(stage_id) {
                    info!(
                        pipeline_stages = %pipeline_stages_progress,
                        stage = %stage_id,
                        checkpoint = %checkpoint.block_number,
                        target = %OptionalField(target),
                        %stage_eta,
                        "Executing stage",
                    );
                } else {
                    info!(
                        pipeline_stages = %pipeline_stages_progress,
                        stage = %stage_id,
                        checkpoint = %checkpoint.block_number,
                        target = %OptionalField(target),
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
                    current_stage.entities_checkpoint = checkpoint.entities();
                    current_stage.eta.update(stage_id, checkpoint);

                    let target = OptionalField(current_stage.target);
                    let stage_progress = current_stage
                        .entities_checkpoint
                        .and_then(|entities| entities.fmt_percentage());
                    let stage_eta = current_stage.eta.fmt_for_stage(stage_id);

                    let message = if done { "Finished stage" } else { "Committed stage progress" };

                    match (stage_progress, stage_eta) {
                        (Some(stage_progress), Some(stage_eta)) => {
                            info!(
                                pipeline_stages = %pipeline_stages_progress,
                                stage = %stage_id,
                                checkpoint = %checkpoint.block_number,
                                %target,
                                %stage_progress,
                                %stage_eta,
                                "{message}",
                            )
                        }
                        (Some(stage_progress), None) => {
                            info!(
                                pipeline_stages = %pipeline_stages_progress,
                                stage = %stage_id,
                                checkpoint = %checkpoint.block_number,
                                %target,
                                %stage_progress,
                                "{message}",
                            )
                        }
                        (None, Some(stage_eta)) => {
                            info!(
                                pipeline_stages = %pipeline_stages_progress,
                                stage = %stage_id,
                                checkpoint = %checkpoint.block_number,
                                %target,
                                %stage_eta,
                                "{message}",
                            )
                        }
                        (None, None) => {
                            info!(
                                pipeline_stages = %pipeline_stages_progress,
                                stage = %stage_id,
                                checkpoint = %checkpoint.block_number,
                                %target,
                                "{message}",
                            )
                        }
                    }
                }

                if done {
                    self.current_stage = None;
                }
            }
            PipelineEvent::Unwind { stage_id, input } => {
                let current_stage = CurrentStage {
                    stage_id,
                    eta: Eta::default(),
                    checkpoint: input.checkpoint,
                    target: Some(input.unwind_to),
                    entities_checkpoint: input.checkpoint.entities(),
                };

                self.current_stage = Some(current_stage);
            }
            _ => (),
        }
    }

    fn handle_network_event(&self, _: NetworkEvent) {
        // NOTE(onbjerg): This used to log established/disconnecting sessions, but this is already
        // logged in the networking component. I kept this stub in case we want to catch other
        // networking events later on.
    }

    fn handle_consensus_engine_event(&mut self, event: BeaconConsensusEngineEvent) {
        match event {
            BeaconConsensusEngineEvent::ForkchoiceUpdated(state, status) => {
                let ForkchoiceState { head_block_hash, safe_block_hash, finalized_block_hash } =
                    state;
                if self.safe_block_hash != Some(safe_block_hash) &&
                    self.finalized_block_hash != Some(finalized_block_hash)
                {
                    let msg = match status {
                        ForkchoiceStatus::Valid => "Forkchoice updated",
                        ForkchoiceStatus::Invalid => "Received invalid forkchoice updated message",
                        ForkchoiceStatus::Syncing => {
                            "Received forkchoice updated message when syncing"
                        }
                    };
                    info!(?head_block_hash, ?safe_block_hash, ?finalized_block_hash, "{}", msg);
                }
                self.head_block_hash = Some(head_block_hash);
                self.safe_block_hash = Some(safe_block_hash);
                self.finalized_block_hash = Some(finalized_block_hash);
            }
            BeaconConsensusEngineEvent::LiveSyncProgress(live_sync_progress) => {
                match live_sync_progress {
                    ConsensusEngineLiveSyncProgress::DownloadingBlocks {
                        remaining_blocks,
                        target,
                    } => {
                        info!(
                            remaining_blocks,
                            target_block_hash=?target,
                            "Live sync in progress, downloading blocks"
                        );
                    }
                }
            }
            BeaconConsensusEngineEvent::CanonicalBlockAdded(block, elapsed) => {
                info!(
                    number=block.number,
                    hash=?block.hash(),
                    peers=self.num_connected_peers(),
                    txs=block.body.transactions.len(),
                    gas=%format_gas(block.header.gas_used),
                    gas_throughput=%format_gas_throughput(block.header.gas_used, elapsed),
                    full=%format!("{:.1}%", block.header.gas_used as f64 * 100.0 / block.header.gas_limit as f64),
                    base_fee=%format!("{:.2}gwei", block.header.base_fee_per_gas.unwrap_or(0) as f64 / constants::GWEI_TO_WEI as f64),
                    blobs=block.header.blob_gas_used.unwrap_or(0) / constants::eip4844::DATA_GAS_PER_BLOB,
                    excess_blobs=block.header.excess_blob_gas.unwrap_or(0) / constants::eip4844::DATA_GAS_PER_BLOB,
                    ?elapsed,
                    "Block added to canonical chain"
                );
            }
            BeaconConsensusEngineEvent::CanonicalChainCommitted(head, elapsed) => {
                self.latest_block = Some(head.number);
                self.latest_block_time = Some(head.timestamp);

                info!(number=head.number, hash=?head.hash(), ?elapsed, "Canonical chain committed");
            }
            BeaconConsensusEngineEvent::ForkBlockAdded(block, elapsed) => {
                info!(number=block.number, hash=?block.hash(), ?elapsed, "Block added to fork chain");
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
                    warn!(?period, "Beacon client online, but no consensus updates received for a while. This may be because of a reth error, or an error in the beacon client! Please investigate reth and beacon client logs!")
                }
            }
        }
    }

    fn handle_pruner_event(&self, event: PrunerEvent) {
        match event {
            PrunerEvent::Started { tip_block_number } => {
                info!(tip_block_number, "Pruner started");
            }
            PrunerEvent::Finished { tip_block_number, elapsed, stats } => {
                info!(tip_block_number, ?elapsed, ?stats, "Pruner finished");
            }
        }
    }

    fn handle_static_file_producer_event(&self, event: StaticFileProducerEvent) {
        match event {
            StaticFileProducerEvent::Started { targets } => {
                info!(?targets, "Static File Producer started");
            }
            StaticFileProducerEvent::Finished { targets, elapsed } => {
                info!(?targets, ?elapsed, "Static File Producer finished");
            }
        }
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
    /// The entities checkpoint for reporting the progress. If `None`, then the progress is not
    /// available, probably because the stage didn't finish running and didn't update its
    /// checkpoint yet.
    entities_checkpoint: Option<EntitiesCheckpoint>,
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
    /// A `static_file_producer` event
    StaticFileProducer(StaticFileProducerEvent),
    /// Used to encapsulate various conditions or situations that do not
    /// naturally fit into the other more specific variants.
    Other(String),
}

impl From<NetworkEvent> for NodeEvent {
    fn from(event: NetworkEvent) -> Self {
        Self::Network(event)
    }
}

impl From<PipelineEvent> for NodeEvent {
    fn from(event: PipelineEvent) -> Self {
        Self::Pipeline(event)
    }
}

impl From<BeaconConsensusEngineEvent> for NodeEvent {
    fn from(event: BeaconConsensusEngineEvent) -> Self {
        Self::ConsensusEngine(event)
    }
}

impl From<ConsensusLayerHealthEvent> for NodeEvent {
    fn from(event: ConsensusLayerHealthEvent) -> Self {
        Self::ConsensusLayerHealth(event)
    }
}

impl From<PrunerEvent> for NodeEvent {
    fn from(event: PrunerEvent) -> Self {
        Self::Pruner(event)
    }
}

impl From<StaticFileProducerEvent> for NodeEvent {
    fn from(event: StaticFileProducerEvent) -> Self {
        Self::StaticFileProducer(event)
    }
}

/// Displays relevant information to the user from components of the node, and periodically
/// displays the high-level status of the node.
pub async fn handle_events<E>(
    peers_info: Option<Box<dyn PeersInfo>>,
    latest_block_number: Option<BlockNumber>,
    events: E,
) where
    E: Stream<Item = NodeEvent> + Unpin,
{
    let state = NodeState::new(peers_info, latest_block_number);

    let start = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut info_interval = tokio::time::interval_at(start, INFO_MESSAGE_INTERVAL);
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
            if let Some(CurrentStage { stage_id, eta, checkpoint, entities_checkpoint, target }) =
                &this.state.current_stage
            {
                let stage_progress =
                    entities_checkpoint.and_then(|entities| entities.fmt_percentage());
                let stage_eta = eta.fmt_for_stage(*stage_id);

                match (stage_progress, stage_eta) {
                    (Some(stage_progress), Some(stage_eta)) => {
                        info!(
                            target: "reth::cli",
                            connected_peers = this.state.num_connected_peers(),
                            stage = %stage_id,
                            checkpoint = checkpoint.block_number,
                            target = %OptionalField(*target),
                            %stage_progress,
                            %stage_eta,
                            "Status"
                        )
                    }
                    (Some(stage_progress), None) => {
                        info!(
                            target: "reth::cli",
                            connected_peers = this.state.num_connected_peers(),
                            stage = %stage_id,
                            checkpoint = checkpoint.block_number,
                            target = %OptionalField(*target),
                            %stage_progress,
                            "Status"
                        )
                    }
                    (None, Some(stage_eta)) => {
                        info!(
                            target: "reth::cli",
                            connected_peers = this.state.num_connected_peers(),
                            stage = %stage_id,
                            checkpoint = checkpoint.block_number,
                            target = %OptionalField(*target),
                            %stage_eta,
                            "Status"
                        )
                    }
                    (None, None) => {
                        info!(
                            target: "reth::cli",
                            connected_peers = this.state.num_connected_peers(),
                            stage = %stage_id,
                            checkpoint = checkpoint.block_number,
                            target = %OptionalField(*target),
                            "Status"
                        )
                    }
                }
            } else if let Some(latest_block) = this.state.latest_block {
                let now =
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                if now - this.state.latest_block_time.unwrap_or(0) > 60 {
                    // Once we start receiving consensus nodes, don't emit status unless stalled for
                    // 1 minute
                    info!(
                        target: "reth::cli",
                        connected_peers = this.state.num_connected_peers(),
                        %latest_block,
                        "Status"
                    );
                }
            } else {
                info!(
                    target: "reth::cli",
                    connected_peers = this.state.num_connected_peers(),
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
                NodeEvent::StaticFileProducer(event) => {
                    this.state.handle_static_file_producer_event(event);
                }
                NodeEvent::Other(event_description) => {
                    warn!("{event_description}");
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
    fn update(&mut self, stage: StageId, checkpoint: StageCheckpoint) {
        let Some(current) = checkpoint.entities() else { return };

        if let Some(last_checkpoint_time) = &self.last_checkpoint_time {
            let Some(processed_since_last) =
                current.processed.checked_sub(self.last_checkpoint.processed)
            else {
                self.eta = None;
                debug!(target: "reth::cli", %stage, ?current, ?self.last_checkpoint, "Failed to calculate the ETA: processed entities is less than the last checkpoint");
                return
            };
            let elapsed = last_checkpoint_time.elapsed();
            let per_second = processed_since_last as f64 / elapsed.as_secs_f64();

            let Some(remaining) = current.total.checked_sub(current.processed) else {
                self.eta = None;
                debug!(target: "reth::cli", %stage, ?current, "Failed to calculate the ETA: total entities is less than processed entities");
                return
            };

            self.eta = Duration::try_from_secs_f64(remaining as f64 / per_second).ok();
        }

        self.last_checkpoint = current;
        self.last_checkpoint_time = Some(Instant::now());
    }

    /// Returns `true` if the ETA is available, i.e. at least one checkpoint has been reported.
    fn is_available(&self) -> bool {
        self.eta.zip(self.last_checkpoint_time).is_some()
    }

    /// Format ETA for a given stage.
    ///
    /// NOTE: Currently ETA is enabled only for the stages that have predictable progress.
    /// It's not the case for network-dependent ([`StageId::Headers`] and [`StageId::Bodies`]) and
    /// [`StageId::Execution`] stages.
    fn fmt_for_stage(&self, stage: StageId) -> Option<String> {
        if !self.is_available() ||
            matches!(stage, StageId::Headers | StageId::Bodies | StageId::Execution)
        {
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
    use super::*;

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
