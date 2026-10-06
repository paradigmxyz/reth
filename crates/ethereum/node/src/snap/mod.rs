//! Runs a snap/2 bootstrap as the engine's backfill.
//!
//! One task alternates header sync and snap state writes while the engine skips forkchoice, so
//! no other writer touches the database. Each pass:
//!
//! 1. Syncs headers to the target; nothing below the pivot executes.
//! 2. Selects a pivot under the head, or resumes the recorded attempt while its pivot is canonical.
//! 3. Downloads state against the pivot's root, carried to newer pivots by block access lists.
//! 4. Hands the state to [`SnapHandoff`], which rebuilds the trie and publishes it at the pivot, or
//!    abandons the attempt on a root mismatch so the next pass starts a new one.
//!
//! A forkchoice update ends the current step at its next boundary, so headers catch up first.
//! The staged pipeline then runs the stages above the pivot.

mod context;
mod handoff;
mod run;

pub use handoff::{HandoffOutcome, RebuildOutcome, SnapHandoff};

use alloy_consensus::BlockHeader;
use alloy_primitives::B256;
use futures::FutureExt;
use reth_chainspec::EthChainSpec;
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync};
use reth_errors::RethError;
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_provider::{
    providers::ProviderNodeTypes, ChainSpecProvider, DatabaseProviderFactory, MetadataProvider,
    ProviderFactory, ProviderResult, StageCheckpointReader,
};
use reth_stages::{Pipeline, PipelineError, PipelineTarget, PipelineWithResult, StageId};
use reth_tasks::Runtime;
use run::SnapRun;
use std::task::{ready, Context, Poll};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::{CancellationToken, DropGuard};

/// Backfills canonical headers, then snap/2 state at a recent pivot.
#[derive(Debug)]
pub struct SnapBackfillSync<N: ProviderNodeTypes, C> {
    // Serves the snap requests and reports peer counts.
    client: C,
    provider_factory: ProviderFactory<N>,
    runtime: Runtime,
    // Owns the pipeline while idle, the result channel while running.
    state: SnapBackfillState<N>,
    // Target queued by the engine, started on the next poll.
    pending_target: Option<PipelineTarget>,
    // Latest finalized block the engine reported, kept across runs.
    finalized: watch::Sender<B256>,
}

impl<N: ProviderNodeTypes, C> SnapBackfillSync<N, C> {
    /// Creates a backfill that has not started any work.
    pub fn new(
        pipeline: Pipeline<N>,
        client: C,
        provider_factory: ProviderFactory<N>,
        runtime: Runtime,
    ) -> Self {
        Self {
            client,
            provider_factory,
            runtime,
            state: SnapBackfillState::Idle(Some(Box::new(pipeline))),
            pending_target: None,
            finalized: watch::channel(B256::ZERO).0,
        }
    }
}

impl<N, C> SnapBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    // Spawns a run if a target is queued and the pipeline is free.
    fn try_spawn(&mut self) -> Option<BackfillEvent> {
        if !matches!(self.state, SnapBackfillState::Idle(_)) {
            return None
        }
        let target = self.pending_target.take()?;
        // Once snap state is verified, or the node was not synced by snap, the staged pipeline
        // backfills alone, including unwinds.
        let event = match (self.needs_snap(), target) {
            // The engine waits for every start to finish, so an unusable target fails the run
            // instead of being dropped.
            (_, PipelineTarget::Sync(hash)) if hash.is_zero() => {
                BackfillEvent::Finished(Err(PipelineError::Internal(RethError::msg(
                    "snap backfill cannot sync to the zero hash",
                ))))
            }
            (Err(error), _) => {
                BackfillEvent::Finished(Err(PipelineError::Internal(RethError::other(error))))
            }
            (Ok(false), target) => self.spawn_staged(target),
            // Nothing executes on top of snap state before the handoff, so there is nothing a
            // snap backfill could unwind.
            (Ok(true), PipelineTarget::Unwind(block)) => {
                BackfillEvent::Finished(Err(PipelineError::Internal(RethError::msg(format!(
                    "snap backfill cannot unwind to block {block}"
                )))))
            }
            (Ok(true), PipelineTarget::Sync(target)) => self.spawn_snap(target),
        };
        Some(event)
    }

    fn spawn_staged(&mut self, target: PipelineTarget) -> BackfillEvent {
        let pipeline = self.take_pipeline();
        let (result_tx, result) = oneshot::channel();
        self.runtime.spawn_critical_blocking_task("pipeline task", async move {
            let _ = result_tx.send(pipeline.run_as_fut(Some(target)).await);
        });
        self.state = SnapBackfillState::Staged(result);
        BackfillEvent::Started(target)
    }

    fn spawn_snap(&mut self, target: B256) -> BackfillEvent {
        let pipeline = self.take_pipeline();
        let (result_tx, result) = oneshot::channel();
        let (targets, target_rx) = watch::channel(target);
        let stop = CancellationToken::new();
        let run = SnapRun::new(
            self.client.clone(),
            self.provider_factory.clone(),
            self.runtime.clone(),
            stop.clone(),
            self.finalized.subscribe(),
        );
        // Node shutdown drops this task as it does the pipeline's; every bootstrap step has either
        // committed or left nothing behind.
        self.runtime.spawn_critical_blocking_task("snap backfill task", async move {
            let _ = result_tx.send(run.run(*pipeline, target_rx).await);
        });
        self.state = SnapBackfillState::Running { _stop: stop.drop_guard(), targets, result };
        BackfillEvent::Started(PipelineTarget::Sync(target))
    }

    fn take_pipeline(&mut self) -> Box<Pipeline<N>> {
        let SnapBackfillState::Idle(pipeline) = &mut self.state else {
            unreachable!("only an idle backfill spawns a run")
        };
        pipeline.take().expect("idle backfill owns its pipeline")
    }

    // Snap bootstraps a node with nothing executed, and finishes any attempt it has not verified
    // yet, including one interrupted after its state was published.
    fn needs_snap(&self) -> ProviderResult<bool> {
        let provider = self.provider_factory.database_provider_ro()?;
        if let Some(attempt) = provider.snap_attempt()? {
            return Ok(!attempt.is_verified())
        }
        // Genesis sets every checkpoint to its own block, which isn't always block 0.
        let genesis = self.provider_factory.chain_spec().genesis_header().number();
        Ok(provider
            .get_stage_checkpoint(StageId::Execution)?
            .is_none_or(|checkpoint| checkpoint.block_number == genesis))
    }
}

impl<N, C> BackfillSync for SnapBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    fn on_action(&mut self, action: BackfillAction) {
        match action {
            // The zero hash never moves a target or anchors a pivot.
            BackfillAction::UpdateTarget(hash) | BackfillAction::UpdateFinalized(hash)
                if hash.is_zero() => {}
            BackfillAction::Start(target) => self.pending_target = Some(target),
            BackfillAction::UpdateTarget(hash) => {
                if let SnapBackfillState::Running { targets, .. } = &self.state {
                    targets.send_if_modified(|current| std::mem::replace(current, hash) != hash);
                }
            }
            // Kept while idle too, so the next run anchors its pivot to known finality.
            BackfillAction::UpdateFinalized(hash) => {
                self.finalized.send_if_modified(|current| std::mem::replace(current, hash) != hash);
            }
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        if let Some(event) = self.try_spawn() {
            return Poll::Ready(event)
        }
        let (SnapBackfillState::Running { result, .. } | SnapBackfillState::Staged(result)) =
            &mut self.state
        else {
            return Poll::Pending
        };
        let event = match ready!(result.poll_unpin(cx)) {
            Ok((pipeline, result)) => {
                self.state = SnapBackfillState::Idle(Some(Box::new(pipeline)));
                BackfillEvent::Finished(result)
            }
            Err(error) => BackfillEvent::TaskDropped(error.to_string()),
        };
        Poll::Ready(event)
    }
}

// Owns the idle pipeline, or the handles of the run holding it.
#[derive(Debug)]
enum SnapBackfillState<N: ProviderNodeTypes> {
    Idle(Option<Box<Pipeline<N>>>),
    Running {
        // Stops the run if the backfill is dropped. Declared first so the run sees the stop
        // before the closed target channel.
        _stop: DropGuard,
        // Latest forkchoice target; older ones are overwritten.
        targets: watch::Sender<B256>,
        // Returns the pipeline with the run's result.
        result: oneshot::Receiver<PipelineWithResult<N>>,
    },
    // The staged pipeline runs alone, over state that is verified or was never snap synced.
    Staged(oneshot::Receiver<PipelineWithResult<N>>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::run::tests::ServesHeaders;
    use alloy_consensus::Header;
    use futures::{future::poll_fn, Stream, StreamExt};
    use reth_provider::{
        test_utils::{
            create_test_provider_factory, create_test_provider_factory_with_genesis_block_number,
            MockNodeTypesWithDB,
        },
        DBProvider, DatabaseProviderFactory, MetadataWriter, StageCheckpointReader,
        StageCheckpointWriter, StorageSettings, StorageSettingsCache,
    };
    use reth_prune::PruneModes;
    use reth_stages::{
        ControlFlow, ExecOutput, PipelineEvent, Stage, StageCheckpoint, StageError, StageId,
        StageSetBuilder,
    };
    use reth_stages_api::test_utils::TestStage;
    use reth_static_file::StaticFileProducer;
    use std::task::Waker;

    type TestBackfill = SnapBackfillSync<MockNodeTypesWithDB, ServesHeaders>;

    type TestProvider =
        <ProviderFactory<MockNodeTypesWithDB> as DatabaseProviderFactory>::ProviderRW;

    // A backfill whose pipeline holds only a scripted header stage, with that pipeline's events.
    fn backfill(headers: TestStage) -> (TestBackfill, impl Stream<Item = PipelineEvent> + Unpin) {
        let (pipeline, factory) = pipeline(headers);
        let events = pipeline.events();
        let backfill =
            SnapBackfillSync::new(pipeline, ServesHeaders::default(), factory, Runtime::test());
        (backfill, events)
    }

    // A pipeline holding only a scripted header stage, over a database in the hashed state
    // layout snap writes into.
    pub(super) fn pipeline(
        headers: TestStage,
    ) -> (Pipeline<MockNodeTypesWithDB>, ProviderFactory<MockNodeTypesWithDB>) {
        pipeline_with(headers, watch::channel(B256::ZERO).0)
    }

    // Like `pipeline`, with any header stage and a tip channel the test can observe.
    pub(super) fn pipeline_with<S: Stage<TestProvider> + 'static>(
        headers: S,
        tip: watch::Sender<B256>,
    ) -> (Pipeline<MockNodeTypesWithDB>, ProviderFactory<MockNodeTypesWithDB>) {
        let factory = hashed_factory();
        (pipeline_on(&factory, StageSetBuilder::default().add_stage(headers), tip), factory)
    }

    // A pipeline over `factory` holding `stages`.
    pub(super) fn pipeline_on(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
        stages: StageSetBuilder<TestProvider>,
        tip: watch::Sender<B256>,
    ) -> Pipeline<MockNodeTypesWithDB> {
        Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stages(stages)
            .with_tip_sender(tip)
            .build(factory.clone(), StaticFileProducer::new(factory.clone(), PruneModes::default()))
    }

    // An empty database in the hashed state layout snap writes into.
    pub(super) fn hashed_factory() -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v2()).unwrap();
        provider.commit().unwrap();
        factory.set_storage_settings_cache(StorageSettings::v2());
        factory
    }

    pub(super) fn headers_done(block: u64) -> Result<ExecOutput, StageError> {
        Ok(ExecOutput { checkpoint: StageCheckpoint::new(block), done: true })
    }

    fn poll_once(backfill: &mut TestBackfill) -> Poll<BackfillEvent> {
        backfill.poll(&mut Context::from_waker(Waker::noop()))
    }

    fn headers_checkpoint(factory: &ProviderFactory<MockNodeTypesWithDB>) -> Option<u64> {
        let provider = factory.database_provider_ro().unwrap();
        provider.get_stage_checkpoint(StageId::Headers).unwrap().map(|it| it.block_number)
    }

    // Waits for the running bootstrap's header stage to commit `block`.
    pub(super) async fn headers_reach(
        events: &mut (impl Stream<Item = PipelineEvent> + Unpin),
        block: u64,
    ) {
        while let Some(event) = events.next().await {
            if let PipelineEvent::Ran { stage_id: StageId::Headers, result, .. } = event &&
                result.checkpoint.block_number == block
            {
                return
            }
        }
        panic!("the pipeline stopped before headers reached {block}")
    }

    pub(super) fn target() -> B256 {
        Header { number: 100, ..Default::default() }.hash_slow()
    }

    pub(super) fn next_target() -> B256 {
        Header { number: 101, ..Default::default() }.hash_slow()
    }

    #[tokio::test]
    async fn executed_state_backfills_with_the_staged_pipeline() {
        let headers = TestStage::new(StageId::Headers).add_exec(headers_done(5));
        let (pipeline, factory) = pipeline(headers);
        let provider = factory.database_provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(42)).unwrap();
        provider.commit().unwrap();
        let mut backfill = SnapBackfillSync::new(
            pipeline,
            ServesHeaders::default(),
            factory.clone(),
            Runtime::test(),
        );

        // Executed state has nothing for snap to download, so the pipeline runs alone.
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(target())));
        assert!(matches!(poll_once(&mut backfill), Poll::Ready(BackfillEvent::Started(_))));
        assert!(matches!(backfill.state, SnapBackfillState::Staged(_)));
        assert!(matches!(poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::Finished(Ok(_))));

        assert_eq!(headers_checkpoint(&factory), Some(5));
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }

    #[test]
    fn a_genesis_above_block_zero_still_snap_syncs() {
        let factory = create_test_provider_factory_with_genesis_block_number(5);
        let provider = factory.database_provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(5)).unwrap();
        provider.commit().unwrap();
        // Only the provider factory decides eligibility, so the pipeline's own database is unused.
        let (pipeline, _) = pipeline(TestStage::new(StageId::Headers));
        let backfill =
            TestBackfill::new(pipeline, ServesHeaders::default(), factory, Runtime::test());

        assert!(backfill.needs_snap().unwrap());
    }

    #[test]
    fn a_zero_hash_target_fails_without_starting_a_run() {
        let (mut backfill, _events) = backfill(TestStage::new(StageId::Headers));

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(B256::ZERO)));

        assert!(matches!(poll_once(&mut backfill), Poll::Ready(BackfillEvent::Finished(Err(_)))));
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }

    #[test]
    fn the_finalized_block_is_kept_while_idle() {
        let (mut backfill, _events) = backfill(TestStage::new(StageId::Headers));
        let finalized = backfill.finalized.subscribe();

        backfill.on_action(BackfillAction::UpdateFinalized(target()));
        backfill.on_action(BackfillAction::UpdateFinalized(B256::ZERO));

        assert_eq!(*finalized.borrow(), target());
        assert!(poll_once(&mut backfill).is_pending());
    }

    #[test]
    fn a_target_update_while_idle_starts_nothing() {
        let (mut backfill, _events) = backfill(TestStage::new(StageId::Headers));

        backfill.on_action(BackfillAction::UpdateTarget(target()));

        assert!(poll_once(&mut backfill).is_pending());
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }

    #[tokio::test]
    async fn an_active_bootstrap_holds_the_pipeline_and_coalesces_targets() {
        let (mut backfill, mut events) =
            backfill(TestStage::new(StageId::Headers).add_exec(headers_done(0)));

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(target())));
        assert!(matches!(
            poll_once(&mut backfill),
            Poll::Ready(BackfillEvent::Started(PipelineTarget::Sync(hash))) if hash == target()
        ));
        headers_reach(&mut events, 0).await;

        let SnapBackfillState::Running { targets, .. } = &backfill.state else {
            panic!("the run owns the pipeline")
        };
        let mut observer = targets.subscribe();
        backfill.on_action(BackfillAction::UpdateTarget(B256::ZERO));
        assert!(!observer.has_changed().unwrap());
        backfill.on_action(BackfillAction::UpdateTarget(next_target()));
        backfill.on_action(BackfillAction::UpdateTarget(next_target()));

        assert!(observer.has_changed().unwrap());
        assert_eq!(*observer.borrow_and_update(), next_target());
        assert!(!observer.has_changed().unwrap());
        // A second start cannot run beside the active one.
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(next_target())));
        assert!(poll_once(&mut backfill).is_pending());
    }

    #[tokio::test]
    async fn a_dropped_backfill_stops_its_run_and_releases_the_pipeline() {
        let (mut backfill, mut events) =
            backfill(TestStage::new(StageId::Headers).add_exec(headers_done(0)));
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(target())));
        assert!(poll_once(&mut backfill).is_ready());
        headers_reach(&mut events, 0).await;

        let SnapBackfillState::Running { _stop, result, .. } =
            std::mem::replace(&mut backfill.state, SnapBackfillState::Idle(None))
        else {
            panic!("the run owns the pipeline")
        };
        drop(_stop);

        let (_pipeline, result) = result.await.unwrap();
        assert!(matches!(result, Ok(ControlFlow::NoProgress { block_number: None })));
    }

    #[tokio::test]
    async fn node_shutdown_keeps_committed_progress() {
        let (mut backfill, mut events) =
            backfill(TestStage::new(StageId::Headers).add_exec(headers_done(0)));
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(target())));
        assert!(poll_once(&mut backfill).is_ready());
        headers_reach(&mut events, 0).await;

        backfill.runtime.graceful_shutdown();

        // The runtime drops the task; the engine treats that as fatal and the node exits.
        assert!(matches!(poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::TaskDropped(_)));
        assert_eq!(headers_checkpoint(&backfill.provider_factory), Some(0));
    }

    #[tokio::test]
    async fn a_failed_run_returns_the_pipeline() {
        let (mut backfill, _events) =
            backfill(TestStage::new(StageId::Headers).add_exec(Err(StageError::ChannelClosed)));
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(target())));
        assert!(poll_once(&mut backfill).is_ready());

        assert!(matches!(poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::Finished(Err(_))));
        // Control and the pipeline are back with the engine, ready for the next target.
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }

    #[tokio::test]
    async fn an_unwind_target_fails_without_starting_a_run() {
        let (mut backfill, _events) = backfill(TestStage::new(StageId::Headers));

        backfill.on_action(BackfillAction::Start(PipelineTarget::Unwind(1)));

        assert!(matches!(poll_once(&mut backfill), Poll::Ready(BackfillEvent::Finished(Err(_)))));
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }
}
