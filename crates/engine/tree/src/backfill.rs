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
use reth_tasks::Runtime;
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

/// A snap sync controller that manages the snap sync lifecycle.
///
/// Snap sync runs in two phases:
///  - **Phase A** – download headers 1..pivot via `HeaderStage` so that static files
///    are populated before any state download begins.
///  - **Phase B** – hand off to the `SnapSyncOrchestrator` for state download.
#[derive(Debug)]
struct SnapSyncController<C, F> {
    client: C,
    factory: F,
    runtime: Runtime,
    /// The target hash that was passed to `start()`, kept around so we can
    /// forward it to the orchestrator after Phase A completes.
    target_hash: Option<alloy_primitives::B256>,
    /// State of the snap sync: `None` = idle, `Some` = running.
    state: Option<SnapSyncState>,
}

/// Running state of snap sync.
#[derive(Debug)]
enum SnapSyncState {
    /// Phase A: downloading headers via `HeaderStage`.
    DownloadingHeaders {
        result_rx: oneshot::Receiver<Result<(), String>>,
    },
    /// Phase B: state download via orchestrator.
    DownloadingState {
        /// Sender for forwarding engine events to the orchestrator.
        /// Kept alive so the orchestrator's receiver doesn't close prematurely.
        #[expect(dead_code)]
        events_tx: tokio::sync::mpsc::UnboundedSender<reth_engine_snap::SnapSyncEvent>,
        /// Receiver for the orchestrator result.
        result_rx: oneshot::Receiver<
            Result<reth_engine_snap::SnapSyncOutcome, reth_engine_snap::SnapSyncError>,
        >,
    },
}

impl<C, F> SnapSyncController<C, F>
where
    C: reth_network_p2p::snap::client::SnapClient
        + reth_network_p2p::block_access_lists::client::BlockAccessListsClient
        + reth_network_p2p::headers::client::HeadersClient<
            Header = <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        >
        + Clone
        + Send
        + Sync
        + 'static,
    F: reth_provider::DatabaseProviderFactory
        + reth_provider::StaticFileProviderFactory
        + reth_storage_api::HeaderSyncGapProvider<
            Header = <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        >
        + Clone
        + Send
        + Sync
        + 'static,
    F::Provider: reth_storage_api::DBProvider
        + reth_provider::HeaderProvider
        + reth_provider::BlockHashReader
        + reth_provider::StorageSettingsCache,
    F::ProviderRW: reth_storage_api::DBProvider
        + reth_provider::StaticFileProviderFactory
        + reth_storage_api::StageCheckpointWriter,
    <F::Provider as reth_storage_api::DBProvider>::Tx: reth_db::transaction::DbTx,
    <F::ProviderRW as reth_storage_api::DBProvider>::Tx: reth_db::transaction::DbTxMut,
    <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader:
        reth_db::table::Value + reth_primitives_traits::FullBlockHeader,
{
    fn new(client: C, factory: F, runtime: Runtime) -> Self {
        Self { client, factory, runtime, target_hash: None, state: None }
    }

    fn is_active(&self) -> bool {
        self.state.is_some()
    }

    /// Start snap sync. Returns `None` because the `SnapSyncStarted` event is
    /// emitted later, once Phase A (header download) completes.
    fn start(&mut self, target_hash: alloy_primitives::B256) -> Option<BackfillEvent> {
        if self.is_active() {
            return None;
        }

        let (result_tx, result_rx) = oneshot::channel();
        let client = self.client.clone();
        let factory = self.factory.clone();

        self.runtime.spawn_critical_blocking_task(
            "snap sync header download",
            async move {
                let result = Self::run_header_stage(client, factory, target_hash).await;
                let _ = result_tx.send(result);
            },
        );

        self.target_hash = Some(target_hash);
        self.state = Some(SnapSyncState::DownloadingHeaders { result_rx });

        // No event yet – we emit SnapSyncStarted after headers finish.
        None
    }

    /// Phase A: resolve the pivot from peers and run `HeaderStage` to fill
    /// static files with headers 1..pivot.
    async fn run_header_stage(
        client: C,
        factory: F,
        target_hash: alloy_primitives::B256,
    ) -> Result<(), String> {
        use alloy_consensus::BlockHeader;
        use reth_config::config::EtlConfig;
        use reth_consensus::noop::NoopConsensus;
        use reth_downloaders::headers::reverse_headers::ReverseHeadersDownloaderBuilder;
        use reth_stages::stages::HeaderStage;
        use reth_stages_api::{ExecInput, Stage, StageCheckpoint, StageExt, StageId};
        use reth_storage_api::{DBProvider, StageCheckpointWriter};

        tracing::info!(target: "sync::snap", %target_hash, "Phase A: resolving target header from peers");

        // 1. Resolve the target header from peers to get block number.
        let target_header = client
            .get_header(alloy_eips::BlockHashOrNumber::Hash(target_hash))
            .await
            .map_err(|e| format!("failed to fetch target header: {e}"))?
            .into_data()
            .ok_or_else(|| "peer returned empty response for target header".to_string())?;
        let target_number = target_header.number();

        // 2. Compute pivot = target - PIVOT_OFFSET.
        let pivot_number = target_number.saturating_sub(reth_engine_snap::PIVOT_OFFSET);
        if pivot_number == 0 {
            tracing::info!(target: "sync::snap", "Target too low for header stage, skipping Phase A");
            return Ok(());
        }

        // 3. Resolve the pivot header from peers to get its hash.
        let pivot_header = client
            .get_header(alloy_eips::BlockHashOrNumber::Number(pivot_number))
            .await
            .map_err(|e| format!("failed to fetch pivot header: {e}"))?
            .into_data()
            .ok_or_else(|| "peer returned empty response for pivot header".to_string())?;
        let pivot_sealed = reth_primitives_traits::SealedHeader::seal_slow(pivot_header);
        let pivot_hash = pivot_sealed.hash();

        tracing::info!(
            target: "sync::snap",
            target_number,
            pivot_number,
            %pivot_hash,
            "Phase A: downloading headers 1..{pivot_number}"
        );

        // 4. Build the tip channel and downloader.
        let (_tip_tx, tip_rx) = tokio::sync::watch::channel(pivot_hash);
        let downloader = ReverseHeadersDownloaderBuilder::default()
            .build(client, NoopConsensus::arc());

        // 5. Build and run the HeaderStage.
        let mut stage = HeaderStage::new(
            factory.clone(),
            downloader,
            tip_rx,
            EtlConfig::default(),
        );

        let input = ExecInput {
            target: Some(pivot_number),
            checkpoint: Some(StageCheckpoint::new(0)),
        };

        // Phase 1 of stage: download headers into ETL (async).
        <HeaderStage<F, _> as StageExt<F::ProviderRW>>::execute_ready(&mut stage, input)
            .await
            .map_err(|e| format!("header download failed: {e}"))?;

        // Phase 2 of stage: write headers to static files + DB (sync).
        let provider_rw =
            factory.database_provider_rw().map_err(|e| format!("db error: {e}"))?;
        let _output = Stage::<F::ProviderRW>::execute(&mut stage, &provider_rw, input)
            .map_err(|e| format!("header write failed: {e}"))?;

        // Save stage checkpoint.
        provider_rw
            .save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(pivot_number))
            .map_err(|e| format!("checkpoint save failed: {e}"))?;

        // Commit.
        provider_rw.commit().map_err(|e| format!("commit failed: {e}"))?;

        use reth_provider::providers::StaticFileWriter;
        factory
            .static_file_provider()
            .commit()
            .map_err(|e| format!("static file commit failed: {e}"))?;

        tracing::info!(target: "sync::snap", pivot_number, "Phase A complete: headers written to static files");
        Ok(())
    }

    /// Spawn the Phase B orchestrator and return the started event.
    fn start_orchestrator(&mut self) -> BackfillEvent {
        let target_hash = self.target_hash.take().expect("target_hash set during start()");
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();

        let orchestrator = reth_engine_snap::orchestrator::SnapSyncOrchestrator::new(
            self.client.clone(),
            self.factory.clone(),
        );

        self.runtime.spawn_critical_blocking_task("snap sync orchestrator", async move {
            let result = orchestrator.run(events_rx, target_hash).await;
            let _ = result_tx.send(result);
        });

        let started_tx = events_tx.clone();
        self.state = Some(SnapSyncState::DownloadingState { events_tx, result_rx });
        BackfillEvent::SnapSyncStarted(started_tx)
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        let Some(state) = &mut self.state else {
            return Poll::Pending;
        };

        match state {
            SnapSyncState::DownloadingHeaders { result_rx } => {
                match result_rx.poll_unpin(cx) {
                    Poll::Ready(Ok(Ok(()))) => {
                        // Phase A succeeded – transition to Phase B.
                        let event = self.start_orchestrator();
                        Poll::Ready(event)
                    }
                    Poll::Ready(Ok(Err(e))) => {
                        self.state = None;
                        self.target_hash = None;
                        Poll::Ready(BackfillEvent::TaskDropped(
                            format!("snap sync header download failed: {e}"),
                        ))
                    }
                    Poll::Ready(Err(_)) => {
                        self.state = None;
                        self.target_hash = None;
                        Poll::Ready(BackfillEvent::TaskDropped(
                            "snap sync header download task dropped".into(),
                        ))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            SnapSyncState::DownloadingState { result_rx, .. } => {
                match result_rx.poll_unpin(cx) {
                    Poll::Ready(Ok(result)) => {
                        self.state = None;
                        self.target_hash = None;
                        Poll::Ready(BackfillEvent::SnapSyncFinished(result))
                    }
                    Poll::Ready(Err(_)) => {
                        self.state = None;
                        self.target_hash = None;
                        Poll::Ready(BackfillEvent::TaskDropped(
                            "snap sync task dropped".into(),
                        ))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

/// Combined backfill sync that supports both pipeline sync and snap sync.
///
/// Only one sync mode can be active at a time.
#[derive(Debug)]
pub struct CombinedBackfillSync<N: ProviderNodeTypes, C, F> {
    pipeline: PipelineSync<N>,
    snap: SnapSyncController<C, F>,
    pending_event: Option<BackfillEvent>,
}

impl<N: ProviderNodeTypes, C, F> CombinedBackfillSync<N, C, F>
where
    C: reth_network_p2p::snap::client::SnapClient
        + reth_network_p2p::block_access_lists::client::BlockAccessListsClient
        + reth_network_p2p::headers::client::HeadersClient<
            Header = <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        >
        + Clone
        + Send
        + Sync
        + 'static,
    F: reth_provider::DatabaseProviderFactory
        + reth_provider::StaticFileProviderFactory
        + reth_storage_api::HeaderSyncGapProvider<
            Header = <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        >
        + Clone
        + Send
        + Sync
        + 'static,
    F::Provider: reth_storage_api::DBProvider
        + reth_provider::HeaderProvider
        + reth_provider::BlockHashReader
        + reth_provider::StorageSettingsCache,
    F::ProviderRW: reth_storage_api::DBProvider
        + reth_provider::StaticFileProviderFactory
        + reth_storage_api::StageCheckpointWriter,
    <F::Provider as reth_storage_api::DBProvider>::Tx: reth_db::transaction::DbTx,
    <F::ProviderRW as reth_storage_api::DBProvider>::Tx: reth_db::transaction::DbTxMut,
    <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader:
        reth_db::table::Value + reth_primitives_traits::FullBlockHeader,
{
    /// Creates a new combined backfill sync.
    pub fn new(
        pipeline: Pipeline<N>,
        pipeline_task_spawner: Runtime,
        client: C,
        factory: F,
        snap_runtime: Runtime,
    ) -> Self {
        Self {
            pipeline: PipelineSync::new(pipeline, pipeline_task_spawner),
            snap: SnapSyncController::new(client, factory, snap_runtime),
            pending_event: None,
        }
    }
}

impl<N: ProviderNodeTypes, C, F> BackfillSync for CombinedBackfillSync<N, C, F>
where
    C: reth_network_p2p::snap::client::SnapClient
        + reth_network_p2p::block_access_lists::client::BlockAccessListsClient
        + reth_network_p2p::headers::client::HeadersClient<
            Header = <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        >
        + Clone
        + Send
        + Sync
        + 'static,
    F: reth_provider::DatabaseProviderFactory
        + reth_provider::StaticFileProviderFactory
        + reth_storage_api::HeaderSyncGapProvider<
            Header = <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        >
        + Clone
        + Send
        + Sync
        + 'static,
    F::Provider: reth_storage_api::DBProvider
        + reth_provider::HeaderProvider
        + reth_provider::BlockHashReader
        + reth_provider::StorageSettingsCache,
    F::ProviderRW: reth_storage_api::DBProvider
        + reth_provider::StaticFileProviderFactory
        + reth_storage_api::StageCheckpointWriter,
    <F::Provider as reth_storage_api::DBProvider>::Tx: reth_db::transaction::DbTx,
    <F::ProviderRW as reth_storage_api::DBProvider>::Tx: reth_db::transaction::DbTxMut,
    <<F::ProviderRW as reth_storage_api::NodePrimitivesProvider>::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader:
        reth_db::table::Value + reth_primitives_traits::FullBlockHeader,
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
                self.pending_event = self.snap.start(target_hash);
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
            return Poll::Ready(event);
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
