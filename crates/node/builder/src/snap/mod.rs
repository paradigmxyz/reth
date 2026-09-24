//! Runs a snap/2 bootstrap as the engine's backfill.
//!
//! Headers and snap state writes alternate in one task while the engine skips forkchoice, so no
//! other writer touches the database during the run.
//!
//! # Lifecycle
//!
//! The engine starts a backfill with a target hash, as it does for the staged pipeline. A run
//! then repeats four steps until the state is downloaded or the run stops:
//!
//! 1. Headers sync to the target, and nothing else: no stage below the pivot may execute over state
//!    the node has not downloaded yet.
//! 2. A pivot is selected under the head, or the attempt an earlier run recorded resumes, while its
//!    pivot is still canonical.
//! 3. Accounts, storage and code download against that pivot's state root and commit as they
//!    arrive, while block access lists carry what is downloaded to a newer pivot as the chain moves
//!    past it.
//! 4. Once every account is covered, the state is handed to the merkle stage.
//!
//! A forkchoice update ends the current step at its next boundary, so headers catch up before
//! the run continues from the progress it committed. Peers that do not serve the pivot's state
//! wait instead of failing the run.
//!
//! The hand-off is where this ends today: [`SnapBackfillSync`] returns an error rather than
//! publishing the state, so nothing executes on state the node has not verified. Publishing it,
//! rebuilding its trie and accepting it come with activation.

mod context;
mod run;

use alloy_primitives::B256;
use futures::FutureExt;
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync};
use reth_errors::RethError;
use reth_network_p2p::snap::client::SnapClient;
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_stages::{Pipeline, PipelineError, PipelineTarget, PipelineWithResult};
use reth_tasks::Runtime;
use run::{SnapRun, HEADER_REFRESH};
use std::task::{ready, Context, Poll};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::{CancellationToken, DropGuard};

/// Backfills canonical headers, then snap/2 state at a recent pivot.
///
/// A new forkchoice target stops the bootstrap at its next step boundary, at most once per
/// header refresh, so headers catch up and the next run resumes the committed attempt.
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
        }
    }
}

impl<N, C> SnapBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + Clone + Unpin + 'static,
{
    // Spawns a run if a target is queued and the pipeline is free.
    fn try_spawn(&mut self) -> Option<BackfillEvent> {
        let SnapBackfillState::Idle(pipeline) = &mut self.state else { return None };
        let target = match self.pending_target.take()? {
            PipelineTarget::Sync(hash) => hash,
            // Nothing executes on top of snap state before activation, so there is nothing a
            // snap backfill could unwind.
            PipelineTarget::Unwind(block) => {
                return Some(BackfillEvent::Finished(Err(PipelineError::Internal(RethError::msg(
                    format!("snap backfill cannot unwind to block {block}"),
                )))))
            }
        };
        let pipeline = pipeline.take().expect("idle backfill owns its pipeline");

        let (result_tx, result) = oneshot::channel();
        let (targets, target_rx) = watch::channel(target);
        let stop = CancellationToken::new();
        let run = SnapRun {
            client: self.client.clone(),
            factory: self.provider_factory.clone(),
            runtime: self.runtime.clone(),
            header_refresh: HEADER_REFRESH,
            stop: stop.clone(),
        };
        // Node shutdown drops this task as it does the pipeline's; every bootstrap step has either
        // committed or left nothing behind.
        self.runtime.spawn_critical_blocking_task("snap backfill task", async move {
            let _ = result_tx.send(run.run(*pipeline, target_rx).await);
        });
        self.state = SnapBackfillState::Running { _stop: stop.drop_guard(), targets, result };

        Some(BackfillEvent::Started(PipelineTarget::Sync(target)))
    }
}

impl<N, C> BackfillSync for SnapBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + Clone + Unpin + 'static,
{
    fn on_action(&mut self, action: BackfillAction) {
        match action {
            // The zero hash is never a usable target.
            BackfillAction::Start(target) | BackfillAction::UpdateTarget(target)
                if target.sync_target().is_some_and(|hash| hash.is_zero()) => {}
            BackfillAction::Start(target) => self.pending_target = Some(target),
            BackfillAction::UpdateTarget(PipelineTarget::Sync(hash)) => {
                if let SnapBackfillState::Running { targets, .. } = &self.state {
                    targets.send_if_modified(|current| {
                        let changed = *current != hash;
                        *current = hash;
                        changed
                    });
                }
            }
            BackfillAction::UpdateTarget(PipelineTarget::Unwind(_)) => {}
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        if let Some(event) = self.try_spawn() {
            return Poll::Ready(event)
        }
        let SnapBackfillState::Running { result, .. } = &mut self.state else {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::poll_fn;
    use reth_network_p2p::NoopFullBlockClient;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        DBProvider, DatabaseProviderFactory, MetadataWriter, StageCheckpointReader,
        StorageSettings, StorageSettingsCache,
    };
    use reth_prune::PruneModes;
    use reth_stages::{ControlFlow, ExecOutput, Stage, StageCheckpoint, StageError, StageId};
    use reth_stages_api::test_utils::TestStage;
    use reth_static_file::StaticFileProducer;
    use std::{task::Waker, time::Duration};

    pub(super) const TARGET: B256 = B256::repeat_byte(1);
    pub(super) const NEXT_TARGET: B256 = B256::repeat_byte(2);

    type TestBackfill = SnapBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient>;

    // A backfill whose pipeline holds only a scripted header stage.
    fn backfill(headers: TestStage) -> TestBackfill {
        let (pipeline, factory) = pipeline(headers);
        SnapBackfillSync::new(pipeline, NoopFullBlockClient::default(), factory, Runtime::test())
    }

    // A pipeline holding only a scripted header stage, over a database in the hashed state
    // layout snap writes into.
    pub(super) fn pipeline(
        headers: TestStage,
    ) -> (Pipeline<MockNodeTypesWithDB>, ProviderFactory<MockNodeTypesWithDB>) {
        pipeline_with(headers, watch::channel(B256::ZERO).0)
    }

    // Like `pipeline`, with any header stage and a tip channel the test can observe.
    pub(super) fn pipeline_with<S>(
        headers: S,
        tip: watch::Sender<B256>,
    ) -> (Pipeline<MockNodeTypesWithDB>, ProviderFactory<MockNodeTypesWithDB>)
    where
        S: Stage<<ProviderFactory<MockNodeTypesWithDB> as DatabaseProviderFactory>::ProviderRW>
            + 'static,
    {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v2()).unwrap();
        provider.commit().unwrap();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .add_stage(headers)
            .with_tip_sender(tip)
            .build(
                factory.clone(),
                StaticFileProducer::new(factory.clone(), PruneModes::default()),
            );
        (pipeline, factory)
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

    // Waits for the running bootstrap's header stage to reach `block`.
    pub(super) async fn headers_reach(factory: &ProviderFactory<MockNodeTypesWithDB>, block: u64) {
        while headers_checkpoint(factory) != Some(block) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[test]
    fn the_zero_hash_is_not_a_usable_target() {
        let mut backfill = backfill(TestStage::new(StageId::Headers));

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(B256::ZERO)));

        assert!(poll_once(&mut backfill).is_pending());
    }

    #[tokio::test]
    async fn an_active_bootstrap_holds_the_pipeline_and_coalesces_targets() {
        let mut backfill = backfill(TestStage::new(StageId::Headers).add_exec(headers_done(0)));

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(TARGET)));
        assert!(matches!(
            poll_once(&mut backfill),
            Poll::Ready(BackfillEvent::Started(PipelineTarget::Sync(TARGET)))
        ));
        headers_reach(&backfill.provider_factory, 0).await;

        let SnapBackfillState::Running { targets, .. } = &backfill.state else {
            panic!("the run owns the pipeline")
        };
        let mut observer = targets.subscribe();
        backfill.on_action(BackfillAction::UpdateTarget(PipelineTarget::Sync(NEXT_TARGET)));
        backfill.on_action(BackfillAction::UpdateTarget(PipelineTarget::Sync(NEXT_TARGET)));

        assert!(observer.has_changed().unwrap());
        assert_eq!(*observer.borrow_and_update(), NEXT_TARGET);
        assert!(!observer.has_changed().unwrap());
        // A second start cannot run beside the active one.
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(NEXT_TARGET)));
        assert!(poll_once(&mut backfill).is_pending());
    }

    #[tokio::test]
    async fn a_dropped_backfill_stops_its_run_and_releases_the_pipeline() {
        let mut backfill = backfill(TestStage::new(StageId::Headers).add_exec(headers_done(0)));
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(TARGET)));
        assert!(poll_once(&mut backfill).is_ready());
        headers_reach(&backfill.provider_factory, 0).await;

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
        let mut backfill = backfill(TestStage::new(StageId::Headers).add_exec(headers_done(0)));
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(TARGET)));
        assert!(poll_once(&mut backfill).is_ready());
        headers_reach(&backfill.provider_factory, 0).await;

        backfill.runtime.graceful_shutdown();

        // The runtime drops the task; the engine treats that as fatal and the node exits.
        assert!(matches!(poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::TaskDropped(_)));
        assert_eq!(headers_checkpoint(&backfill.provider_factory), Some(0));
    }

    #[tokio::test]
    async fn a_failed_run_returns_the_pipeline() {
        let mut backfill =
            backfill(TestStage::new(StageId::Headers).add_exec(Err(StageError::ChannelClosed)));
        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(TARGET)));
        assert!(poll_once(&mut backfill).is_ready());

        assert!(matches!(poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::Finished(Err(_))));
        // Control and the pipeline are back with the engine, ready for the next target.
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }

    #[tokio::test]
    async fn an_unwind_target_fails_without_starting_a_run() {
        let mut backfill = backfill(TestStage::new(StageId::Headers));

        backfill.on_action(BackfillAction::Start(PipelineTarget::Unwind(1)));

        assert!(matches!(poll_once(&mut backfill), Poll::Ready(BackfillEvent::Finished(Err(_)))));
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }
}
