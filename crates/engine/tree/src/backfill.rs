//! It is expected that the node has two sync modes:
//!
//!  - Backfill sync: Sync to a certain block height in stages, e.g. download data from p2p then
//!    execute that range.
//!  - Live sync: In this mode the node is keeping up with the latest tip and listens for new
//!    requests from the consensus client.
//!
//! These modes are mutually exclusive and the node can only be in one mode at a time.

use futures::FutureExt;
use reth_engine_snap::controller::{SnapSyncControl, SnapSyncControlEvent, SnapSyncController};
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_stages_api::{ControlFlow, Pipeline, PipelineError, PipelineTarget, PipelineWithResult};
use reth_tasks::Runtime;
use std::{
    fmt,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tracing::trace;

/// Represents the state of the backfill synchronization process.
#[derive(Debug, PartialEq, Eq, Default)]
pub enum BackfillSyncState {
    /// The node is not performing any backfill synchronization.
    /// This is the initial or default state.
    #[default]
    Idle,
    /// A backfill synchronization has been requested or planned, but processing has not started
    /// yet.
    Pending,
    /// The node is actively engaged in backfill synchronization.
    Active,
}

impl BackfillSyncState {
    /// Returns true if the state is idle.
    pub const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Returns true if the state is pending.
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Returns true if the state is active.
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}

/// Backfill sync mode functionality.
pub trait BackfillSync: Send {
    /// Performs a backfill action.
    fn on_action(&mut self, action: BackfillAction);

    /// Polls the pipeline for completion.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent>;
}

/// The backfill actions that can be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillAction {
    /// Start backfilling with the given target.
    Start(PipelineTarget),
    /// Start snap sync (for fresh nodes with no state).
    ///
    /// Carries the target block hash from the FCU so the orchestrator can resolve
    /// the head from peers if it's not available locally.
    StartSnapSync(alloy_primitives::B256),
}

/// The events that can be emitted on backfill sync.
#[derive(Debug)]
pub enum BackfillEvent {
    /// Backfill sync started.
    Started(PipelineTarget),
    /// Backfill sync finished.
    ///
    /// If this is returned, backfill sync is idle.
    Finished(Result<ControlFlow, PipelineError>),
    /// Sync task was dropped after it was started, unable to receive it because
    /// channel closed. This would indicate a panicked task.
    TaskDropped(String),
    /// Snap sync started. Contains the event sender for forwarding chain events.
    SnapSyncStarted(tokio::sync::mpsc::UnboundedSender<reth_engine_snap::SnapSyncEvent>),
    /// Snap sync finished.
    SnapSyncFinished(Result<reth_engine_snap::SnapSyncOutcome, reth_engine_snap::SnapSyncError>),
}

/// Pipeline sync.
#[derive(Debug)]
pub struct PipelineSync<N: ProviderNodeTypes> {
    /// The type that can spawn the pipeline task.
    pipeline_task_spawner: Runtime,
    /// The current state of the pipeline.
    /// The pipeline is used for large ranges.
    pipeline_state: PipelineState<N>,
    /// Pending target block for the pipeline to sync
    pending_pipeline_target: Option<PipelineTarget>,
}

impl<N: ProviderNodeTypes> PipelineSync<N> {
    /// Create a new instance.
    pub fn new(pipeline: Pipeline<N>, pipeline_task_spawner: Runtime) -> Self {
        Self {
            pipeline_task_spawner,
            pipeline_state: PipelineState::Idle(Some(Box::new(pipeline))),
            pending_pipeline_target: None,
        }
    }

    /// Returns `true` if a pipeline target is queued and will be triggered on the next `poll`.
    #[expect(dead_code)]
    const fn is_pipeline_sync_pending(&self) -> bool {
        self.pending_pipeline_target.is_some() && self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is idle.
    const fn is_pipeline_idle(&self) -> bool {
        self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is active.
    pub(crate) const fn is_pipeline_active(&self) -> bool {
        !self.is_pipeline_idle()
    }

    /// Sets a new target to sync the pipeline to.
    ///
    /// But ensures the target is not the zero hash.
    fn set_pipeline_sync_target(&mut self, target: PipelineTarget) {
        if target.sync_target().is_some_and(|target| target.is_zero()) {
            trace!(
                target: "consensus::engine::sync",
                "Pipeline target cannot be zero hash."
            );
            // precaution to never sync to the zero hash
            return
        }
        self.pending_pipeline_target = Some(target);
    }

    /// This will spawn the pipeline if it is idle and a target is set or if the pipeline is set to
    /// run continuously.
    fn try_spawn_pipeline(&mut self) -> Option<BackfillEvent> {
        match &mut self.pipeline_state {
            PipelineState::Idle(pipeline) => {
                let target = self.pending_pipeline_target.take()?;
                let (tx, rx) = oneshot::channel();

                let pipeline = pipeline.take().expect("exists");
                self.pipeline_task_spawner.spawn_critical_blocking_task(
                    "pipeline task",
                    async move {
                        let result = pipeline.run_as_fut(Some(target)).await;
                        let _ = tx.send(result);
                    },
                );
                self.pipeline_state = PipelineState::Running(rx);

                Some(BackfillEvent::Started(target))
            }
            PipelineState::Running(_) => None,
        }
    }

    /// Advances the pipeline state.
    ///
    /// This checks for the result in the channel, or returns pending if the pipeline is idle.
    fn poll_pipeline(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        let res = match self.pipeline_state {
            PipelineState::Idle(_) => return Poll::Pending,
            PipelineState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };
        let ev = match res {
            Ok((pipeline, result)) => {
                self.pipeline_state = PipelineState::Idle(Some(Box::new(pipeline)));
                BackfillEvent::Finished(result)
            }
            Err(why) => {
                // failed to receive the pipeline
                BackfillEvent::TaskDropped(why.to_string())
            }
        };
        Poll::Ready(ev)
    }
}

impl<N: ProviderNodeTypes> BackfillSync for PipelineSync<N> {
    fn on_action(&mut self, event: BackfillAction) {
        match event {
            BackfillAction::Start(target) => self.set_pipeline_sync_target(target),
            BackfillAction::StartSnapSync(_) => {}
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        // try to spawn a pipeline if a target is set
        if let Some(event) = self.try_spawn_pipeline() {
            return Poll::Ready(event)
        }

        // make sure we poll the pipeline if it's active, and return any ready pipeline events
        if self.is_pipeline_active() {
            // advance the pipeline
            if let Poll::Ready(event) = self.poll_pipeline(cx) {
                return Poll::Ready(event)
            }
        }

        Poll::Pending
    }
}

/// The possible pipeline states within the sync controller.
///
/// [`PipelineState::Idle`] means that the pipeline is currently idle.
/// [`PipelineState::Running`] means that the pipeline is currently running.
///
/// NOTE: The differentiation between these two states is important, because when the pipeline is
/// running, it acquires the write lock over the database. This means that we cannot forward to the
/// blockchain tree any messages that would result in database writes, since it would result in a
/// deadlock.
#[derive(Debug)]
enum PipelineState<N: ProviderNodeTypes> {
    /// Pipeline is idle.
    Idle(Option<Box<Pipeline<N>>>),
    /// Pipeline is running and waiting for a response
    Running(oneshot::Receiver<PipelineWithResult<N>>),
}

impl<N: ProviderNodeTypes> PipelineState<N> {
    /// Returns `true` if the state matches idle.
    const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle(_))
    }
}

/// Combined backfill sync that supports both pipeline sync and snap sync.
///
/// Only one sync mode can be active at a time.
pub struct CombinedBackfillSync<N: ProviderNodeTypes, S> {
    pipeline: PipelineSync<N>,
    snap: S,
    pending_event: Option<BackfillEvent>,
}

/// Combined backfill sync using the default provider factory snap controller.
pub type EngineBackfillSync<N, C> =
    CombinedBackfillSync<N, SnapSyncController<C, ProviderFactory<N>>>;

impl<N, S> fmt::Debug for CombinedBackfillSync<N, S>
where
    N: ProviderNodeTypes,
    S: SnapSyncControl,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CombinedBackfillSync")
            .field("pipeline_active", &self.pipeline.is_pipeline_active())
            .field("snap_active", &self.snap.is_active())
            .field("pending_event", &self.pending_event)
            .finish()
    }
}

impl<N, S> CombinedBackfillSync<N, S>
where
    N: ProviderNodeTypes,
    S: SnapSyncControl,
{
    /// Creates a new combined backfill sync with the given snap sync adapter.
    pub fn with_snap(pipeline: PipelineSync<N>, snap: S) -> Self {
        Self { pipeline, snap, pending_event: None }
    }
}

impl<N, C, F> CombinedBackfillSync<N, SnapSyncController<C, F>>
where
    N: ProviderNodeTypes,
    SnapSyncController<C, F>: SnapSyncControl,
{
    /// Creates a new combined backfill sync.
    pub fn new(
        pipeline: Pipeline<N>,
        pipeline_task_spawner: Runtime,
        client: C,
        factory: F,
        snap_runtime: Runtime,
    ) -> Self {
        Self::with_snap(
            PipelineSync::new(pipeline, pipeline_task_spawner),
            SnapSyncController::new(client, factory, snap_runtime),
        )
    }
}

impl<N, S> BackfillSync for CombinedBackfillSync<N, S>
where
    N: ProviderNodeTypes,
    S: SnapSyncControl,
{
    fn on_action(&mut self, action: BackfillAction) {
        match action {
            BackfillAction::Start(target) => {
                if self.snap.is_active() {
                    tracing::warn!(target: "consensus::engine::sync", "Ignoring pipeline start while snap sync is active");
                    return;
                }
                self.pipeline.on_action(BackfillAction::Start(target));
            }
            BackfillAction::StartSnapSync(target_hash) => {
                if self.pipeline.is_pipeline_active() {
                    tracing::warn!(target: "consensus::engine::sync", "Ignoring snap sync start while pipeline is active");
                    return;
                }
                self.snap.start(target_hash);
                self.pending_event = None;
            }
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        // Return any pending event first (e.g. snap sync started)
        if let Some(event) = self.pending_event.take() {
            return Poll::Ready(event);
        }

        // Poll snap sync
        if let Poll::Ready(event) = self.snap.poll(cx) {
            return Poll::Ready(match event {
                SnapSyncControlEvent::Started(events_tx) => {
                    BackfillEvent::SnapSyncStarted(events_tx)
                }
                SnapSyncControlEvent::Finished(result) => BackfillEvent::SnapSyncFinished(result),
                SnapSyncControlEvent::TaskDropped(err) => BackfillEvent::TaskDropped(err),
            });
        }

        // Poll pipeline
        self.pipeline.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{insert_headers_into_client, TestPipelineBuilder};
    use alloy_consensus::Header;
    use alloy_eips::eip1559::ETHEREUM_BLOCK_GAS_LIMIT_30M;
    use alloy_primitives::{BlockNumber, B256};
    use assert_matches::assert_matches;
    use futures::poll;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_primitives_traits::SealedHeader;
    use reth_provider::test_utils::MockNodeTypesWithDB;
    use reth_stages::ExecOutput;
    use reth_stages_api::StageCheckpoint;
    use reth_tasks::Runtime;
    use std::{collections::VecDeque, future::poll_fn, sync::Arc};

    struct TestHarness {
        pipeline_sync: PipelineSync<MockNodeTypesWithDB>,
        tip: B256,
    }

    impl TestHarness {
        fn new(total_blocks: usize, pipeline_done_after: u64) -> Self {
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            // force the pipeline to be "done" after `pipeline_done_after` blocks
            let pipeline = TestPipelineBuilder::new()
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(BlockNumber::from(pipeline_done_after)),
                    done: true,
                })]))
                .build(chain_spec);

            let pipeline_sync = PipelineSync::new(pipeline, Runtime::test());
            let client = TestFullBlockClient::default();
            let header = Header {
                base_fee_per_gas: Some(7),
                gas_limit: ETHEREUM_BLOCK_GAS_LIMIT_30M,
                ..Default::default()
            };
            let header = SealedHeader::seal_slow(header);
            insert_headers_into_client(&client, header, 0..total_blocks);

            let tip = client.highest_block().expect("there should be blocks here").hash();

            Self { pipeline_sync, tip }
        }
    }

    #[tokio::test]
    async fn pipeline_started_and_finished() {
        const TOTAL_BLOCKS: usize = 10;
        const PIPELINE_DONE_AFTER: u64 = 5;
        let TestHarness { mut pipeline_sync, tip } =
            TestHarness::new(TOTAL_BLOCKS, PIPELINE_DONE_AFTER);

        let sync_future = poll_fn(|cx| pipeline_sync.poll(cx));
        let next_event = poll!(sync_future);

        // sync target not set, pipeline not started
        assert_matches!(next_event, Poll::Pending);

        pipeline_sync.on_action(BackfillAction::Start(PipelineTarget::Sync(tip)));

        let sync_future = poll_fn(|cx| pipeline_sync.poll(cx));
        let next_event = poll!(sync_future);

        // sync target set, pipeline started
        assert_matches!(next_event, Poll::Ready(BackfillEvent::Started(target)) => {
            assert_eq!(target.sync_target().unwrap(), tip);
        });

        // the next event should be the pipeline finishing in a good state
        let sync_future = poll_fn(|cx| pipeline_sync.poll(cx));
        let next_ready = sync_future.await;
        assert_matches!(next_ready, BackfillEvent::Finished(result) => {
            assert_matches!(result, Ok(control_flow) => assert_eq!(control_flow, ControlFlow::Continue { block_number: PIPELINE_DONE_AFTER }));
        });
    }
}
