//! It is expected that the node has two sync modes:
//!
//!  - Backfill sync: Sync to a certain block height in stages, e.g. download data from p2p then
//!    execute that range.
//!  - Live sync: In this mode the node is keeping up with the latest tip and listens for new
//!    requests from the consensus client.
//!
//! These modes are mutually exclusive and the node can only be in one mode at a time.

use futures::FutureExt;
use reth_provider::providers::ProviderNodeTypes;
use reth_stages_api::{ControlFlow, Pipeline, PipelineError, PipelineTarget, PipelineWithResult};
use reth_tasks::TaskSpawner;
use std::task::{ready, Context, Poll};
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
pub trait BackfillSync: Send + Sync {
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
}

/// Pipeline sync.
#[derive(Debug)]
pub struct PipelineSync<N: ProviderNodeTypes> {
    /// The type that can spawn the pipeline task.
    pipeline_task_spawner: Box<dyn TaskSpawner>,
    /// The current state of the pipeline.
    /// The pipeline is used for large ranges.
    pipeline_state: PipelineState<N>,
    /// Pending target block for the pipeline to sync
    pending_pipeline_target: Option<PipelineTarget>,
}

impl<N: ProviderNodeTypes> PipelineSync<N> {
    /// Create a new instance.
    pub fn new(pipeline: Pipeline<N>, pipeline_task_spawner: Box<dyn TaskSpawner>) -> Self {
        Self {
            pipeline_task_spawner,
            pipeline_state: PipelineState::Idle(Some(pipeline)),
            pending_pipeline_target: None,
        }
    }

    /// Returns `true` if a pipeline target is queued and will be triggered on the next `poll`.
    #[allow(dead_code)]
    const fn is_pipeline_sync_pending(&self) -> bool {
        self.pending_pipeline_target.is_some() && self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is idle.
    const fn is_pipeline_idle(&self) -> bool {
        self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is active.
    const fn is_pipeline_active(&self) -> bool {
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
                self.pipeline_task_spawner.spawn_critical_blocking(
                    "pipeline task",
                    Box::pin(async move {
                        let result = pipeline.run_as_fut(Some(target)).await;
                        let _ = tx.send(result);
                    }),
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
                self.pipeline_state = PipelineState::Idle(Some(pipeline));
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
    Idle(Option<Pipeline<N>>),
    /// Pipeline is running and waiting for a response
    Running(oneshot::Receiver<PipelineWithResult<N>>),
}

impl<N: ProviderNodeTypes> PipelineState<N> {
    /// Returns `true` if the state matches idle.
    const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{insert_headers_into_client, TestPipelineBuilder};
    use alloy_primitives::{BlockNumber, Sealable, B256};
    use assert_matches::assert_matches;
    use futures::poll;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_primitives::{Header, SealedHeader};
    use reth_provider::test_utils::MockNodeTypesWithDB;
    use reth_stages::ExecOutput;
    use reth_stages_api::StageCheckpoint;
    use reth_tasks::TokioTaskExecutor;
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
                .build(chain_spec.clone());

            let pipeline_sync = PipelineSync::new(pipeline, Box::<TokioTaskExecutor>::default());
            let client = TestFullBlockClient::default();
            let sealed = Header {
                base_fee_per_gas: Some(7),
                gas_limit: chain_spec.max_gas_limit,
                ..Default::default()
            }
            .seal_slow();
            let (header, seal) = sealed.into_parts();
            let header = SealedHeader::new(header, seal);
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
