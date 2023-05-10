//! Sync management for the engine implementation.

use futures::FutureExt;
use reth_db::database::Database;
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    full_block::{FetchFullBlockFuture, FullBlockClient},
    headers::client::HeadersClient,
};
use reth_primitives::{SealedBlock, H256};
use reth_stages::{ControlFlow, Pipeline, PipelineError, PipelineWithResult};
use reth_tasks::TaskSpawner;
use std::{
    collections::VecDeque,
    future::Future,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;

/// Manages syncing under the control of the engine.
///
/// This type controls the [Pipeline] and supports full block downloads.
///
/// Caution: If the pipeline is running, this type will not emit blocks downloaded from the network
/// [EngineSyncEvent::FetchedFullBlock] until the pipeline is idle to prevent commits to the
/// database while the pipeline is still active.
pub(crate) struct EngineSyncManager<Client, DB>
where
    Client: HeadersClient + BodiesClient,
    DB: Database,
{
    /// A downloader that can download full blocks from the network.
    full_block_client: FullBlockClient<Client>,
    /// The type that can spawn the pipeline task.
    pipeline_task_spawner: Box<dyn TaskSpawner>,
    /// The current state of the pipeline.
    /// The pipeline is used for large ranges.
    pipeline_state: PipelineState<DB>,
    /// Pending target for the pipeline.
    pending_pipeline_target: Option<PipelineSyncTarget>,
    /// In requests in progress.
    inflight_full_block_requests: Vec<FetchFullBlockFuture<Client>>,
    /// Buffered events until the manager is polled and the pipeline is idle.
    queued_events: VecDeque<EngineSyncEvent>,
}

impl<Client, DB> EngineSyncManager<Client, DB>
where
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
    DB: Database + 'static,
{
    /// Create a new instance
    pub(crate) fn new(
        client: Client,
        pipeline: Pipeline<DB>,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        Self {
            full_block_client: FullBlockClient::new(client),
            pipeline_task_spawner,
            pipeline_state: PipelineState::Idle(Some(pipeline)),
            pending_pipeline_target: None,
            inflight_full_block_requests: Vec::new(),
            queued_events: VecDeque::new(),
        }
    }

    /// Cancels all full block requests that are in progress.
    pub(crate) fn clear_full_block_requests(&mut self) {
        self.inflight_full_block_requests.clear();
    }

    /// Returns `true` if the pipeline is idle.
    pub(crate) fn is_pipeline_idle(&self) -> bool {
        self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is active.
    pub(crate) fn is_pipeline_syncing(&self) -> bool {
        !self.pipeline_state.is_idle()
    }

    /// Starts requesting a full block from the network.
    pub(crate) fn download_full_block(&mut self, hash: H256) {
        let request = self.full_block_client.get_full_block(hash);
        self.inflight_full_block_requests.push(request);
    }

    /// Sets a new target to sync the pipeline to.
    pub(crate) fn set_pipeline_target(&mut self, target: PipelineSyncTarget) {
        self.pending_pipeline_target = Some(target);
    }

    /// Advances the pipeline state.
    ///
    /// This checks for the result in the channel, or returns pending if the pipeline is idle.
    fn poll_pipeline(&mut self, cx: &mut Context<'_>) -> Poll<EngineSyncEvent> {
        let res = match self.pipeline_state {
            PipelineState::Idle(_) => return Poll::Pending,
            PipelineState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };
        let ev = match res {
            Ok((pipeline, res)) => {
                self.pipeline_state = PipelineState::Idle(Some(pipeline));
                EngineSyncEvent::PipelineFinished(res)
            }
            Err(_) => {
                // failed to receive the pipeline
                EngineSyncEvent::PipelineTaskDropped
            }
        };
        Poll::Ready(ev)
    }

    fn try_spawn_pipeline(&mut self) -> Option<EngineSyncEvent> {
        match &mut self.pipeline_state {
            PipelineState::Idle(pipeline) => {
                let target = self.pending_pipeline_target.take()?.0;
                let (tx, rx) = oneshot::channel();

                let mut pipeline = pipeline.take().expect("exists");
                self.pipeline_task_spawner.spawn_critical_blocking(
                    "pipeline task",
                    Box::pin(async move {
                        let result = pipeline.run_as_fut(Some(target)).await;
                        let _ = tx.send(result);
                    }),
                );
                self.pipeline_state = PipelineState::Running(rx);
                Some(EngineSyncEvent::PipelineStarted(target))
            }
            PipelineState::Running(_) => None,
        }
    }

    /// Advances the sync process.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<EngineSyncEvent> {
        // try to spawn a pipeline if a target is set
        if let Some(event) = self.try_spawn_pipeline() {
            return Poll::Ready(event)
        }

        loop {
            // drain buffered events first if pipeline is not running
            if self.is_pipeline_idle() {
                if let Some(event) = self.queued_events.pop_front() {
                    return Poll::Ready(event)
                }
            } else {
                // advance the pipeline
                if let Poll::Ready(event) = self.poll_pipeline(cx) {
                    return Poll::Ready(event)
                }
            }

            // advance all requests
            for idx in (0..self.inflight_full_block_requests.len()).rev() {
                let mut request = self.inflight_full_block_requests.swap_remove(idx);
                if let Poll::Ready(block) = request.poll_unpin(cx) {
                    self.queued_events.push_back(EngineSyncEvent::FetchedFullBlock(block));
                } else {
                    // still pending
                    self.inflight_full_block_requests.push(request);
                }
            }

            if !self.pipeline_state.is_idle() || self.queued_events.is_empty() {
                // can not make any progress
                return Poll::Pending
            }
        }
    }
}

/// The event type emitted by the [EngineSyncManager].
#[derive(Debug)]
pub(crate) enum EngineSyncEvent {
    /// A full block has been downloaded from the network.
    FetchedFullBlock(SealedBlock),
    /// Pipeline started syncing
    PipelineStarted(H256),
    /// Pipeline finished
    ///
    /// If this is returned, the pipeline is idle.
    PipelineFinished(Result<ControlFlow, PipelineError>),
    /// Pipeline was dropped after it was started
    PipelineTaskDropped,
}

/// The target hash to pass to the pipeline.
#[derive(Debug)]
pub(crate) struct PipelineSyncTarget(pub H256);

/// The possible pipeline states within the sync controller.
///
/// [PipelineState::Idle] means that the pipeline is currently idle.
/// [PipelineState::Running] means that the pipeline is currently running.
///
/// NOTE: The differentiation between these two states is important, because when the pipeline is
/// running, it acquires the write lock over the database. This means that we cannot forward to the
/// blockchain tree any messages that would result in database writes, since it would result in a
/// deadlock.
enum PipelineState<DB: Database> {
    /// Pipeline is idle.
    Idle(Option<Pipeline<DB>>),
    /// Pipeline is running and waiting for a response
    Running(oneshot::Receiver<PipelineWithResult<DB>>),
}

impl<DB: Database> PipelineState<DB> {
    /// Returns `true` if the state matches idle.
    fn is_idle(&self) -> bool {
        matches!(self, PipelineState::Idle(_))
    }
}
