//! Runs snap bootstrap under the engine's exclusive backfill database access.
//! Canonical headers precede state download; ordinary stages resume above the published pivot.

use super::{context::NodeSnapContext, handoff::publish_snap_state};
use alloy_eips::BlockNumHash;
use futures::FutureExt;
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync, PipelineSync};
use reth_errors::RethError;
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, MetadataProvider,
    ProviderFactory, ProviderResult, StageCheckpointReader, StageCheckpointWriter,
};
use reth_snap_sync::{SnapBootstrap, SnapBootstrapOutcome, SnapPivotPolicy, SnapStateVerifier};
use reth_stages::stages::MerkleStage;
use reth_stages_api::{
    ExecInput, Pipeline, PipelineError, PipelineTarget, PipelineWithResult, Stage, StageId,
};
use reth_tasks::Runtime;
use std::task::{ready, Context, Poll};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Bootstraps headers, state and remaining stages before reporting backfill completion.
/// Databases with executed or verified state delegate to [`PipelineSync`].
#[derive(Debug)]
pub struct SnapBackfillSync<N: ProviderNodeTypes, C> {
    /// Serves the snap requests, and reports peer counts to the sync.
    client: C,
    /// Every phase of the bootstrap writes through this factory.
    provider_factory: ProviderFactory<N>,
    /// Spawns the bootstrap off the orchestrator's poll loop.
    task_spawner: Runtime,
    /// Decides where attempts are anchored.
    policy: SnapPivotPolicy,
    /// Owns the pipeline while idle, the result channel while running.
    state: SnapBackfillState<N>,
    /// Target requested while a run was already in flight.
    pending_target: Option<PipelineTarget>,
    /// Coalesces target updates while the bootstrap owns the pipeline.
    active_target: Option<watch::Sender<PipelineTarget>>,
}

impl<N: ProviderNodeTypes, C> SnapBackfillSync<N, C> {
    /// Creates a backfill that has not started any work.
    pub fn new(
        pipeline: Pipeline<N>,
        client: C,
        provider_factory: ProviderFactory<N>,
        task_spawner: Runtime,
    ) -> Self {
        Self {
            client,
            provider_factory,
            task_spawner,
            policy: SnapPivotPolicy::default(),
            state: SnapBackfillState::Idle(Some(Box::new(pipeline))),
            pending_target: None,
            active_target: None,
        }
    }

    /// Sets the pivot distance and history bounds the bootstrap enforces.
    pub const fn with_policy(mut self, policy: SnapPivotPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Queues a target, ignoring the zero hash the engine can hand out before it knows a tip.
    fn set_target(&mut self, target: PipelineTarget) {
        if target.sync_target().is_some_and(|target| target.is_zero()) {
            debug!(target: "sync::snap", "Snap backfill target cannot be zero hash");
            return
        }
        self.pending_target = Some(target);
    }
}

impl<N, C> SnapBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + Send + Sync + 'static,
{
    /// Spawns one bootstrap if a target is queued and the pipeline is free.
    fn try_spawn(&mut self) -> Option<BackfillEvent> {
        let SnapBackfillState::Idle(pipeline) = &mut self.state else { return None };
        let target = self.pending_target.take()?;
        match needs_snap(&self.provider_factory) {
            Ok(true) => {}
            Ok(false) => {
                let pipeline = pipeline.take().expect("idle backfill owns its pipeline");
                let mut sync = PipelineSync::new(*pipeline, self.task_spawner.clone());
                sync.on_action(BackfillAction::Start(target));
                self.state = SnapBackfillState::Pipeline(sync);
                return None
            }
            Err(error) => {
                return Some(BackfillEvent::Finished(Err(PipelineError::Internal(
                    RethError::other(error),
                ))))
            }
        }
        let pipeline = pipeline.take().expect("idle backfill owns its pipeline");

        let (tx, rx) = oneshot::channel();
        let (target_tx, target_rx) = watch::channel(target);
        self.active_target = Some(target_tx);
        let client = self.client.clone();
        let provider_factory = self.provider_factory.clone();
        let runtime = self.task_spawner.clone();
        let policy = self.policy;

        self.task_spawner.spawn_critical_blocking_task("snap backfill task", async move {
            let result =
                bootstrap(*pipeline, client, provider_factory, runtime, policy, target_rx).await;
            let _ = tx.send(result);
        });
        self.state = SnapBackfillState::Running(rx);

        Some(BackfillEvent::Started(target))
    }

    /// Returns the bootstrap's outcome once the whole sequence has finished.
    fn poll_bootstrap(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        let SnapBackfillState::Running(rx) = &mut self.state else { return Poll::Pending };
        let event = match ready!(rx.poll_unpin(cx)) {
            Ok((pipeline, result)) => {
                self.active_target = None;
                self.state = SnapBackfillState::Idle(Some(Box::new(pipeline)));
                BackfillEvent::Finished(result)
            }
            Err(error) => BackfillEvent::TaskDropped(error.to_string()),
        };
        Poll::Ready(event)
    }
}

impl<N, C> BackfillSync for SnapBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + Send + Sync + 'static,
{
    fn on_action(&mut self, action: BackfillAction) {
        if let SnapBackfillState::Pipeline(sync) = &mut self.state {
            sync.on_action(action);
            return
        }
        match action {
            BackfillAction::Start(target) => self.set_target(target),
            BackfillAction::UpdateTarget(target) => {
                if target.sync_target().is_some_and(|hash| hash.is_zero()) {
                    return
                }
                if let Some(sender) = &self.active_target {
                    sender.send_if_modified(|current| {
                        if *current == target {
                            return false
                        }
                        *current = target;
                        true
                    });
                }
            }
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        if let Some(event) = self.try_spawn() {
            return Poll::Ready(event)
        }
        match &mut self.state {
            SnapBackfillState::Pipeline(sync) => sync.poll(cx),
            SnapBackfillState::Running(_) => self.poll_bootstrap(cx),
            SnapBackfillState::Idle(_) => Poll::Pending,
        }
    }
}

// Owns the idle pipeline or the running bootstrap, which holds the database write lock.
#[derive(Debug)]
enum SnapBackfillState<N: ProviderNodeTypes> {
    /// No bootstrap in flight; the pipeline is parked here.
    Idle(Option<Box<Pipeline<N>>>),
    /// A bootstrap is running and will return the pipeline with its result.
    Running(oneshot::Receiver<PipelineWithResult<N>>),
    /// Snap bootstrap is no longer needed; ordinary backfill owns the pipeline permanently.
    Pipeline(PipelineSync<N>),
}

// Snap bootstraps a node with nothing executed, and finishes any attempt it has not verified yet,
// including one interrupted after its state was published.
fn needs_snap<N: ProviderNodeTypes>(factory: &ProviderFactory<N>) -> ProviderResult<bool> {
    let provider = factory.database_provider_ro()?;
    if let Some(attempt) = provider.snap_attempt()? {
        return Ok(!attempt.is_verified())
    }
    Ok(provider.get_stage_checkpoint(StageId::Execution)?.unwrap_or_default().block_number == 0)
}

/// Runs headers, then the state download and its trie rebuild, then the stages above the pivot.
async fn bootstrap<N, C>(
    mut pipeline: Pipeline<N>,
    client: C,
    provider_factory: ProviderFactory<N>,
    runtime: Runtime,
    policy: SnapPivotPolicy,
    mut targets: watch::Receiver<PipelineTarget>,
) -> PipelineWithResult<N>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + Send + Sync + 'static,
{
    let mut target;
    let outcome = loop {
        target = *targets.borrow_and_update();
        // Snap needs canonical headers and their BAL commitments, but nothing below the pivot may
        // be executed, so only the header prefix of the pipeline runs first.
        if let Err(error) = pipeline.run_until(StageId::Headers, Some(target)).await {
            return (pipeline, Err(error))
        }

        let cancel = CancellationToken::new();
        let context = NodeSnapContext::new(provider_factory.clone(), client.clone());
        let mut session =
            SnapBootstrap::new(client.clone(), provider_factory.clone(), runtime.clone(), context)
                .with_policy(policy)
                .with_cancellation(cancel.clone());
        let mut run = std::pin::pin!(session.run());
        let (outcome, updated) = tokio::select! {
            biased;
            changed = targets.changed() => {
                // Let in-flight writes commit before headers take the database again.
                cancel.cancel();
                (run.await, changed.is_ok())
            }
            outcome = &mut run => (outcome, false),
        };

        match outcome {
            Ok(SnapBootstrapOutcome::Stopped) if updated => {}
            Ok(SnapBootstrapOutcome::Stopped) => {
                return (
                    pipeline,
                    Err(PipelineError::Internal(RethError::msg(
                        "snap bootstrap stopped before its state was verified",
                    ))),
                )
            }
            Ok(outcome) => break outcome,
            Err(error) => return (pipeline, Err(PipelineError::Internal(RethError::other(error)))),
        }
    };

    if let SnapBootstrapOutcome::TrieRebuild { write, pivot } = outcome {
        let factory = provider_factory.clone();
        // Publishing and the trie rebuild read every account, so they run on the blocking pool.
        let activated = runtime
            .spawn_blocking(move || -> Result<(), PipelineError> {
                let provider = factory.database_provider_rw()?;
                publish_snap_state(&provider, pivot.number)?;
                provider.commit()?;
                rebuild_trie(&factory, pivot)?;
                let provider = factory.database_provider_rw()?;
                provider
                    .verify_state_root(write)
                    .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
                provider.commit()?;
                Ok(())
            })
            .await
            .map_err(|error| PipelineError::Internal(RethError::other(error)))
            .and_then(|result| result);
        if let Err(error) = activated {
            return (pipeline, Err(error))
        }
        info!(target: "sync::snap", ?pivot, "Snap state verified, resuming the pipeline above it");
    }

    // Stages the published state satisfies skip straight to the pivot, so this only executes
    // what is genuinely missing above it.
    pipeline.run_as_fut(Some(target)).await
}

// Rebuilds the trie from the downloaded state up to `pivot`, committing the stage's progress in
// chunks so a restart resumes it. The stage checks the root against the pivot's header.
fn rebuild_trie<N: ProviderNodeTypes>(
    factory: &ProviderFactory<N>,
    pivot: BlockNumHash,
) -> Result<(), PipelineError> {
    let mut stage = MerkleStage::default_execution();
    loop {
        let provider = factory.database_provider_rw()?;
        let checkpoint = provider.get_stage_checkpoint(StageId::MerkleExecute)?;
        let output =
            stage.execute(&provider, ExecInput { target: Some(pivot.number), checkpoint })?;
        provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint)?;
        provider.commit()?;
        if output.done {
            return Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_db_api::models::SnapAttempt;
    use reth_network_p2p::NoopFullBlockClient;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        DatabaseProviderFactory, MetadataWriter, StorageSettings, StorageSettingsCache,
    };
    use reth_prune::PruneModes;
    use reth_stages_api::{ControlFlow, StageCheckpoint};
    use reth_static_file::StaticFileProducer;
    use reth_storage_api::metadata::keys;
    use std::task::Waker;

    fn backfill() -> SnapBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient> {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v2()).unwrap();
        provider.commit().unwrap();
        let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .with_tip_sender(tokio::sync::watch::channel(B256::ZERO).0)
            .build(
                factory.clone(),
                StaticFileProducer::new(factory.clone(), PruneModes::default()),
            );
        SnapBackfillSync::new(pipeline, NoopFullBlockClient::default(), factory, Runtime::test())
    }

    // Records an attempt anchored at block 100, verified when `verified` is set.
    fn record_attempt(
        backfill: &SnapBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient>,
        verified: bool,
    ) {
        let provider = backfill.provider_factory.database_provider_rw().unwrap();
        let mut attempt = SnapAttempt::start(
            None,
            BlockNumHash::new(100, B256::repeat_byte(1)),
            B256::repeat_byte(2),
        );
        if verified {
            attempt.verify();
        }
        provider.write_snap_attempt(&attempt).unwrap();
        provider.commit().unwrap();
    }

    fn poll_once(
        backfill: &mut SnapBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient>,
    ) -> Poll<BackfillEvent> {
        backfill.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn an_idle_backfill_without_a_target_stays_pending() {
        let mut backfill = backfill();

        assert!(poll_once(&mut backfill).is_pending());
    }

    #[test]
    fn target_updates_only_notify_an_active_bootstrap() {
        let mut backfill = backfill();
        let first = PipelineTarget::Sync(B256::repeat_byte(1));
        let next = PipelineTarget::Sync(B256::repeat_byte(2));
        backfill.on_action(BackfillAction::UpdateTarget(next));
        assert!(backfill.pending_target.is_none());

        let (sender, mut receiver) = watch::channel(first);
        backfill.active_target = Some(sender);
        backfill.on_action(BackfillAction::UpdateTarget(next));
        assert!(receiver.has_changed().unwrap());
        assert_eq!(*receiver.borrow_and_update(), next);
        backfill.on_action(BackfillAction::UpdateTarget(next));
        assert!(!receiver.has_changed().unwrap());
        assert!(backfill.pending_target.is_none());
    }

    #[test]
    fn the_zero_hash_is_not_a_usable_target() {
        let mut backfill = backfill();

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(B256::ZERO)));

        // Nothing was queued, so polling cannot spawn a bootstrap towards it.
        assert!(backfill.pending_target.is_none());
        assert!(poll_once(&mut backfill).is_pending());
    }

    #[test]
    fn a_real_target_starts_one_bootstrap() {
        let mut backfill = backfill();
        let target = PipelineTarget::Sync(B256::repeat_byte(1));

        backfill.on_action(BackfillAction::Start(target));
        let event = poll_once(&mut backfill);

        assert!(matches!(event, Poll::Ready(BackfillEvent::Started(started)) if started == target));
        // The pipeline moved into the running bootstrap, so no second run can start beside it.
        assert!(matches!(backfill.state, SnapBackfillState::Running(_)));
    }

    #[tokio::test]
    async fn executed_or_verified_state_uses_ordinary_backfill() {
        for verified_attempt in [false, true] {
            let mut backfill = backfill();
            if verified_attempt {
                record_attempt(&backfill, true);
            } else {
                let provider = backfill.provider_factory.database_provider_rw().unwrap();
                provider
                    .save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(42))
                    .unwrap();
                provider.commit().unwrap();
            }

            let target = PipelineTarget::Sync(B256::repeat_byte(1));
            backfill.on_action(BackfillAction::Start(target));
            assert!(
                matches!(poll_once(&mut backfill), Poll::Ready(BackfillEvent::Started(started)) if started == target)
            );
            assert!(matches!(backfill.state, SnapBackfillState::Pipeline(_)));

            // A target queued during ordinary backfill stays with `PipelineSync` rather than
            // starting a snap bootstrap once the current run finishes.
            let next_target = PipelineTarget::Sync(B256::repeat_byte(2));
            backfill.on_action(BackfillAction::Start(next_target));
            assert!(matches!(
                futures::future::poll_fn(|cx| backfill.poll(cx)).await,
                BackfillEvent::Finished(Ok(_))
            ));
            assert!(
                matches!(futures::future::poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::Started(started) if started == next_target)
            );
            assert!(matches!(
                futures::future::poll_fn(|cx| backfill.poll(cx)).await,
                BackfillEvent::Finished(Ok(_))
            ));
        }
    }

    #[tokio::test]
    async fn a_verified_bootstrap_hands_the_pending_target_to_the_pipeline() {
        let mut backfill = backfill();
        let SnapBackfillState::Idle(ref mut pipeline) = backfill.state else { unreachable!() };
        let pipeline = pipeline.take().unwrap();
        let (tx, rx) = oneshot::channel();
        backfill.state = SnapBackfillState::Running(rx);
        let target = PipelineTarget::Sync(B256::repeat_byte(2));
        backfill.on_action(BackfillAction::Start(target));

        // The bootstrap task verifies its state before returning the pipeline.
        record_attempt(&backfill, true);
        tx.send((*pipeline, Ok(ControlFlow::NoProgress { block_number: None }))).unwrap();

        assert!(matches!(
            futures::future::poll_fn(|cx| backfill.poll(cx)).await,
            BackfillEvent::Finished(Ok(_))
        ));
        assert!(
            matches!(futures::future::poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::Started(started) if started == target)
        );
        assert!(matches!(backfill.state, SnapBackfillState::Pipeline(_)));
    }

    #[test]
    fn an_unverified_attempt_keeps_the_snap_path_after_publication() {
        let mut backfill = backfill();
        record_attempt(&backfill, false);
        // Published state moves execution to the pivot before the trie is verified.
        let provider = backfill.provider_factory.database_provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(100)).unwrap();
        provider.commit().unwrap();

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(B256::repeat_byte(3))));

        assert!(matches!(poll_once(&mut backfill), Poll::Ready(BackfillEvent::Started(_))));
        assert!(matches!(backfill.state, SnapBackfillState::Running(_)));
    }

    #[test]
    fn an_unreadable_attempt_starts_neither_sync_path() {
        let mut backfill = backfill();
        let provider = backfill.provider_factory.database_provider_rw().unwrap();
        provider.write_metadata(keys::SNAP_ATTEMPT, vec![0xff]).unwrap();
        provider.commit().unwrap();

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(B256::repeat_byte(1))));

        assert!(matches!(poll_once(&mut backfill), Poll::Ready(BackfillEvent::Finished(Err(_)))));
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }
}
