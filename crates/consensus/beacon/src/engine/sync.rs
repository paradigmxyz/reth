//! Sync management for the engine implementation.

use crate::engine::metrics::EngineSyncMetrics;
use futures::FutureExt;
use reth_db::database::Database;
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    full_block::{FetchFullBlockFuture, FetchFullBlockRangeFuture, FullBlockClient},
    headers::client::HeadersClient,
};
use reth_primitives::{BlockNumber, SealedBlock, H256};
use reth_stages::{ControlFlow, Pipeline, PipelineError, PipelineWithResult};
use reth_tasks::TaskSpawner;
use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tracing::trace;

/// Manages syncing under the control of the engine.
///
/// This type controls the [Pipeline] and supports (single) full block downloads.
///
/// Caution: If the pipeline is running, this type will not emit blocks downloaded from the network
/// [EngineSyncEvent::FetchedFullBlock] until the pipeline is idle to prevent commits to the
/// database while the pipeline is still active.
pub(crate) struct EngineSyncController<DB, Client>
where
    DB: Database,
    Client: HeadersClient + BodiesClient,
{
    /// A downloader that can download full blocks from the network.
    full_block_client: FullBlockClient<Client>,
    /// The type that can spawn the pipeline task.
    pipeline_task_spawner: Box<dyn TaskSpawner>,
    /// The current state of the pipeline.
    /// The pipeline is used for large ranges.
    pipeline_state: PipelineState<DB>,
    /// Pending target block for the pipeline to sync
    pending_pipeline_target: Option<H256>,
    /// In-flight full block requests in progress.
    inflight_full_block_requests: Vec<FetchFullBlockFuture<Client>>,
    /// In-flight full block _range_ requests in progress.
    inflight_block_range_requests: Vec<FetchFullBlockRangeFuture<Client>>,
    /// Buffered blocks from downloads - this is a min-heap of blocks, using the block number for
    /// ordering. This means the blocks will be popped from the heap with ascending block numbers.
    range_buffered_blocks: BinaryHeap<Reverse<OrderedSealedBlock>>,
    /// If enabled, the pipeline will be triggered continuously, as soon as it becomes idle
    run_pipeline_continuously: bool,
    /// Max block after which the consensus engine would terminate the sync. Used for debugging
    /// purposes.
    max_block: Option<BlockNumber>,
    /// Engine sync metrics.
    metrics: EngineSyncMetrics,
}

impl<DB, Client> EngineSyncController<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    /// Create a new instance
    pub(crate) fn new(
        pipeline: Pipeline<DB>,
        client: Client,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
        run_pipeline_continuously: bool,
        max_block: Option<BlockNumber>,
    ) -> Self {
        Self {
            full_block_client: FullBlockClient::new(client),
            pipeline_task_spawner,
            pipeline_state: PipelineState::Idle(Some(pipeline)),
            pending_pipeline_target: None,
            inflight_full_block_requests: Vec::new(),
            inflight_block_range_requests: Vec::new(),
            range_buffered_blocks: BinaryHeap::new(),
            run_pipeline_continuously,
            max_block,
            metrics: EngineSyncMetrics::default(),
        }
    }

    /// Sets the metrics for the active downloads
    fn update_block_download_metrics(&self) {
        self.metrics.active_block_downloads.set(self.inflight_full_block_requests.len() as f64);
        // TODO: full block range metrics
    }

    /// Sets the max block value for testing
    #[cfg(test)]
    pub(crate) fn set_max_block(&mut self, block: BlockNumber) {
        self.max_block = Some(block);
    }

    /// Cancels all full block requests that are in progress.
    pub(crate) fn clear_full_block_requests(&mut self) {
        self.inflight_full_block_requests.clear();
        self.update_block_download_metrics();
    }

    /// Cancels the full block request with the given hash.
    pub(crate) fn cancel_full_block_request(&mut self, hash: H256) {
        self.inflight_full_block_requests.retain(|req| *req.hash() != hash);
        self.update_block_download_metrics();
    }

    /// Returns whether or not the sync controller is set to run the pipeline continuously.
    pub(crate) fn run_pipeline_continuously(&self) -> bool {
        self.run_pipeline_continuously
    }

    /// Returns `true` if a pipeline target is queued and will be triggered on the next `poll`.
    #[allow(unused)]
    pub(crate) fn is_pipeline_sync_pending(&self) -> bool {
        self.pending_pipeline_target.is_some() && self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is idle.
    pub(crate) fn is_pipeline_idle(&self) -> bool {
        self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is active.
    pub(crate) fn is_pipeline_active(&self) -> bool {
        !self.is_pipeline_idle()
    }

    /// Returns true if there's already a request for the given hash.
    pub(crate) fn is_inflight_request(&self, hash: H256) -> bool {
        self.inflight_full_block_requests.iter().any(|req| *req.hash() == hash)
    }

    /// Starts requesting a range of blocks from the network, in reverse from the given hash.
    ///
    /// If the `count` is 1, this will use the `download_full_block` method instead, because it
    /// downloads headers and bodies for the block concurrently.
    pub(crate) fn download_block_range(&mut self, hash: H256, count: u64) {
        if count == 1 {
            self.download_full_block(hash);
        } else {
            trace!(
                target: "consensus::engine",
                ?hash,
                ?count,
                "start downloading full block range."
            );

            let request = self.full_block_client.get_full_block_range(hash, count);
            self.inflight_block_range_requests.push(request);
        }

        // // TODO: need more metrics for block ranges
        // self.update_block_download_metrics();
    }

    /// Starts requesting a full block from the network.
    ///
    /// Returns `true` if the request was started, `false` if there's already a request for the
    /// given hash.
    pub(crate) fn download_full_block(&mut self, hash: H256) -> bool {
        if self.is_inflight_request(hash) {
            return false
        }
        trace!(
            target: "consensus::engine",
            ?hash,
            "start downloading full block."
        );
        let request = self.full_block_client.get_full_block(hash);
        self.inflight_full_block_requests.push(request);

        self.update_block_download_metrics();

        true
    }

    /// Sets a new target to sync the pipeline to.
    pub(crate) fn set_pipeline_sync_target(&mut self, target: H256) {
        self.pending_pipeline_target = Some(target);
    }

    /// Check if the engine reached max block as specified by `max_block` parameter.
    ///
    /// Note: this is mainly for debugging purposes.
    pub(crate) fn has_reached_max_block(&self, progress: BlockNumber) -> bool {
        let has_reached_max_block =
            self.max_block.map(|target| progress >= target).unwrap_or_default();
        if has_reached_max_block {
            trace!(
                target: "consensus::engine",
                ?progress,
                max_block = ?self.max_block,
                "Consensus engine reached max block."
            );
        }
        has_reached_max_block
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
            Ok((pipeline, result)) => {
                let minimum_block_number = pipeline.minimum_block_number();
                let reached_max_block =
                    self.has_reached_max_block(minimum_block_number.unwrap_or_default());
                self.pipeline_state = PipelineState::Idle(Some(pipeline));
                EngineSyncEvent::PipelineFinished { result, reached_max_block }
            }
            Err(_) => {
                // failed to receive the pipeline
                EngineSyncEvent::PipelineTaskDropped
            }
        };
        Poll::Ready(ev)
    }

    /// This will spawn the pipeline if it is idle and a target is set or if the pipeline is set to
    /// run continuously.
    fn try_spawn_pipeline(&mut self) -> Option<EngineSyncEvent> {
        match &mut self.pipeline_state {
            PipelineState::Idle(pipeline) => {
                let target = self.pending_pipeline_target.take();

                if target.is_none() && !self.run_pipeline_continuously {
                    // nothing to sync
                    return None
                }

                let (tx, rx) = oneshot::channel();

                let pipeline = pipeline.take().expect("exists");
                self.pipeline_task_spawner.spawn_critical_blocking(
                    "pipeline task",
                    Box::pin(async move {
                        let result = pipeline.run_as_fut(target).await;
                        let _ = tx.send(result);
                    }),
                );
                self.pipeline_state = PipelineState::Running(rx);

                // we also clear any pending full block requests because we expect them to be
                // outdated (included in the range the pipeline is syncing anyway)
                self.clear_full_block_requests();

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

        // make sure we poll the pipeline if it's active, and return any ready pipeline events
        if !self.is_pipeline_idle() {
            // advance the pipeline
            if let Poll::Ready(event) = self.poll_pipeline(cx) {
                return Poll::Ready(event)
            }
        }

        // advance all full block requests
        for idx in (0..self.inflight_full_block_requests.len()).rev() {
            let mut request = self.inflight_full_block_requests.swap_remove(idx);
            if let Poll::Ready(block) = request.poll_unpin(cx) {
                trace!(target: "consensus::engine", block=?block.num_hash(), "Received single full block, buffering");
                self.range_buffered_blocks.push(Reverse(OrderedSealedBlock(block)));
            } else {
                // still pending
                self.inflight_full_block_requests.push(request);
            }
        }

        // advance all full block range requests
        for idx in (0..self.inflight_block_range_requests.len()).rev() {
            let mut request = self.inflight_block_range_requests.swap_remove(idx);
            if let Poll::Ready(blocks) = request.poll_unpin(cx) {
                trace!(target: "consensus::engine", len=?blocks.len(), first=?blocks.first().map(|b| b.num_hash()), last=?blocks.last().map(|b| b.num_hash()), "Received full block range, buffering");
                self.range_buffered_blocks
                    .extend(blocks.into_iter().map(OrderedSealedBlock).map(Reverse));
            } else {
                // still pending
                self.inflight_block_range_requests.push(request);
            }
        }

        self.update_block_download_metrics();

        // drain an element of the block buffer if there are any
        if let Some(block) = self.range_buffered_blocks.pop() {
            return Poll::Ready(EngineSyncEvent::FetchedFullBlock(block.0 .0))
        }

        Poll::Pending
    }
}

/// A wrapper type around [SealedBlock] that implements the [Ord] trait by block number.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderedSealedBlock(SealedBlock);

impl PartialOrd for OrderedSealedBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.0.number.partial_cmp(&other.0.number)
    }
}

impl Ord for OrderedSealedBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.number.cmp(&other.0.number)
    }
}

/// The event type emitted by the [EngineSyncController].
#[derive(Debug)]
pub(crate) enum EngineSyncEvent {
    /// A full block has been downloaded from the network.
    FetchedFullBlock(SealedBlock),
    /// Pipeline started syncing
    ///
    /// This is none if the pipeline is triggered without a specific target.
    PipelineStarted(Option<H256>),
    /// Pipeline finished
    ///
    /// If this is returned, the pipeline is idle.
    PipelineFinished {
        /// Final result of the pipeline run.
        result: Result<ControlFlow, PipelineError>,
        /// Whether the pipeline reached the configured `max_block`.
        ///
        /// Note: this is only relevant in debugging scenarios.
        reached_max_block: bool,
    },
    /// Pipeline task was dropped after it was started, unable to receive it because channel
    /// closed. This would indicate a panicked pipeline task
    PipelineTaskDropped,
}

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
