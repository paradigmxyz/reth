//! Snapshot bootstrap wired into the engine's backfill slot.
//!
//! Snap assembles state from peers instead of executing history, but it still needs canonical
//! headers first and the ordinary stages afterwards. Running it as a [`BackfillSync`] gives that
//! sequence the same exclusive database access the staged pipeline gets, so the engine buffers
//! payloads for the whole bootstrap rather than racing it.

use futures::FutureExt;
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync, PipelineSync};
use reth_errors::RethError;
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_snap_sync::{
    NodeSnapContext, SnapBootstrap, SnapPivotPolicy, SnapStateStore, SnapSyncOutcome,
};
use reth_stages_api::{Pipeline, PipelineError, PipelineTarget, PipelineWithResult, StageId};
use reth_tasks::{shutdown::signal, Runtime};
use std::task::{ready, Context, Poll};
use tokio::sync::{oneshot, watch};
use tracing::{debug, info};

/// Drives a snapshot bootstrap through the engine's backfill interface.
///
/// One backfill run is headers, then state download, then the remaining stages above the published
/// pivot. [`BackfillEvent::Finished`] is only emitted once all three are done, so the engine never
/// treats the persisted chain as ready while stage bookkeeping is still inconsistent.
/// Once state is published, subsequent backfills delegate to [`PipelineSync`]. Databases with
/// existing execution progress use [`PipelineSync`] without starting a snapshot.
#[derive(Debug)]
pub struct SnapBackfillSync<N: ProviderNodeTypes, C> {
    /// Serves the snap requests, and reports peer counts to the session.
    client: C,
    /// Every phase of the bootstrap writes through this factory.
    provider_factory: ProviderFactory<N>,
    /// Spawns the bootstrap off the orchestrator's poll loop.
    task_spawner: Runtime,
    /// Decides where generations are anchored.
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
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    /// Spawns one bootstrap if a target is queued and the pipeline is free.
    fn try_spawn(&mut self) -> Option<BackfillEvent> {
        let SnapBackfillState::Idle(pipeline) = &mut self.state else { return None };
        let target = self.pending_target.take()?;
        match SnapStateStore::new(&self.provider_factory).requires_bootstrap() {
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
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
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

/// Owns the pipeline while idle and the bootstrap's result channel while running.
///
/// The distinction matters for the same reason it does for the staged pipeline: a running
/// bootstrap holds the database write lock, so no other component may write while it is active.
#[derive(Debug)]
enum SnapBackfillState<N: ProviderNodeTypes> {
    /// No bootstrap in flight; the pipeline is parked here.
    Idle(Option<Box<Pipeline<N>>>),
    /// A bootstrap is running and will return the pipeline with its result.
    Running(oneshot::Receiver<PipelineWithResult<N>>),
    /// Snapshot bootstrap is no longer needed; ordinary backfill owns the pipeline permanently.
    Pipeline(PipelineSync<N>),
}

/// Runs headers, then the state download, then the stages above the published pivot.
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
    C: SnapClient + HeadersClient,
{
    let mut target;
    loop {
        target = *targets.borrow_and_update();
        // Snap needs canonical headers and their BAL commitments, but nothing below the pivot may
        // be executed, so only the header prefix of the pipeline runs first.
        if let Err(error) = pipeline.run_until(StageId::Headers, Some(target)).await {
            return (pipeline, Err(error))
        }

        // The session borrows locals, so the bootstrap owns the clones it was handed.
        let (signal, shutdown) = signal();
        let context = NodeSnapContext::new(&provider_factory, &client, shutdown);
        let mut session = SnapBootstrap::new(&client, &provider_factory, context, runtime.clone())
            .with_policy(policy);
        let mut run = std::pin::pin!(session.run());
        let (outcome, updated) = tokio::select! {
            biased;
            changed = targets.changed() => {
                let updated = changed.is_ok();
                if updated {
                    target = *targets.borrow_and_update();
                }
                // Let in-flight verification and writes finish before headers take the database.
                signal.fire();
                (run.await, updated)
            }
            outcome = &mut run => (outcome, false),
        };

        match outcome {
            Ok(SnapSyncOutcome::Complete { generation }) => {
                info!(
                    target: "sync::snap",
                    block_number = generation.target_block,
                    "Snap state published, resuming the pipeline above the pivot"
                );
                break
            }
            Ok(SnapSyncOutcome::Stalled { generation }) => {
                debug!(
                    target: "sync::snap",
                    resumable = generation.is_some(),
                    generation = ?generation,
                    "Snap state download made no further progress"
                );
                if !updated {
                    return (
                        pipeline,
                        Err(PipelineError::Internal(RethError::msg(
                            "Snap bootstrap interrupted before state publication",
                        ))),
                    )
                }
            }
            Err(error) => return (pipeline, Err(PipelineError::Internal(RethError::other(error)))),
        }
    }

    // Stages the published frontier satisfies skip straight to the pivot, so this only executes
    // what is genuinely missing above it.
    pipeline.run_as_fut(Some(target)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_network_p2p::NoopFullBlockClient;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        DBProvider, DatabaseProviderFactory, StageCheckpointWriter, StorageSettings,
        StorageSettingsCache,
    };
    use reth_prune::PruneModes;
    use reth_stages_api::{ControlFlow, StageCheckpoint};
    use reth_static_file::StaticFileProducer;
    use std::task::Waker;

    fn backfill() -> SnapBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient> {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .with_tip_sender(tokio::sync::watch::channel(B256::ZERO).0)
            .build(
                factory.clone(),
                StaticFileProducer::new(factory.clone(), PruneModes::default()),
            );
        SnapBackfillSync::new(pipeline, NoopFullBlockClient::default(), factory, Runtime::test())
    }

    #[test]
    fn an_idle_backfill_without_a_target_stays_pending() {
        let mut backfill = backfill();
        let waker = Waker::noop();

        assert!(backfill.poll(&mut Context::from_waker(waker)).is_pending());
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
        assert!(backfill.poll(&mut Context::from_waker(Waker::noop())).is_pending());
    }

    #[test]
    fn a_real_target_starts_one_bootstrap() {
        let mut backfill = backfill();
        let target = PipelineTarget::Sync(B256::repeat_byte(1));

        backfill.on_action(BackfillAction::Start(target));
        let event = backfill.poll(&mut Context::from_waker(Waker::noop()));

        assert!(matches!(event, Poll::Ready(BackfillEvent::Started(started)) if started == target));
        // The pipeline moved into the running bootstrap, so no second run can start beside it.
        assert!(matches!(backfill.state, SnapBackfillState::Running(_)));
    }

    #[tokio::test]
    async fn existing_state_uses_ordinary_backfill() {
        for stage in [StageId::Execution, StageId::Finish, StageId::Other("SnapSync")] {
            let mut backfill = backfill();
            let provider = backfill.provider_factory.database_provider_rw().unwrap();
            provider.save_stage_checkpoint(stage, StageCheckpoint::new(42)).unwrap();
            provider.commit().unwrap();

            let target = PipelineTarget::Sync(B256::repeat_byte(1));
            backfill.on_action(BackfillAction::Start(target));
            assert!(
                matches!(backfill.poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(BackfillEvent::Started(started)) if started == target)
            );
            assert!(matches!(backfill.state, SnapBackfillState::Pipeline(_)));

            // A target queued during ordinary backfill must stay with PipelineSync, not start
            // another snapshot after the current run finishes.
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
            assert!(SnapStateStore::new(&backfill.provider_factory)
                .interrupted_generation()
                .unwrap()
                .is_none());
        }
    }

    #[tokio::test]
    async fn completed_bootstrap_hands_pending_target_to_pipeline() {
        let mut backfill = backfill();
        let SnapBackfillState::Idle(ref mut pipeline) = backfill.state else { unreachable!() };
        let pipeline = pipeline.take().unwrap();
        let (tx, rx) = oneshot::channel();
        backfill.state = SnapBackfillState::Running(rx);
        let target = PipelineTarget::Sync(B256::repeat_byte(2));
        backfill.on_action(BackfillAction::Start(target));

        // Model the atomic publication performed by the snapshot task before it returns its
        // pipeline. The wrapper must observe the durable marker on the same instance.
        let provider = backfill.provider_factory.database_provider_rw().unwrap();
        provider
            .save_stage_checkpoint(StageId::Other("SnapSync"), StageCheckpoint::new(42))
            .unwrap();
        provider.commit().unwrap();
        tx.send((*pipeline, Ok(ControlFlow::NoProgress { block_number: None }))).unwrap();

        assert!(matches!(
            futures::future::poll_fn(|cx| backfill.poll(cx)).await,
            BackfillEvent::Finished(Ok(_))
        ));
        assert!(
            matches!(futures::future::poll_fn(|cx| backfill.poll(cx)).await, BackfillEvent::Started(started) if started == target)
        );
        assert!(matches!(backfill.state, SnapBackfillState::Pipeline(_)));
        assert!(matches!(
            futures::future::poll_fn(|cx| backfill.poll(cx)).await,
            BackfillEvent::Finished(Ok(_))
        ));
    }

    #[test]
    fn interrupted_bootstrap_keeps_the_snapshot_path() {
        let mut backfill = backfill();
        let store = SnapStateStore::new(&backfill.provider_factory);
        let generation = reth_snap_sync::SnapDownloadProgress::new(
            100,
            B256::repeat_byte(1),
            B256::repeat_byte(2),
        );
        store.begin_generation(generation).unwrap();

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(B256::repeat_byte(3))));
        assert!(matches!(
            backfill.poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(BackfillEvent::Started(_))
        ));
        assert!(matches!(backfill.state, SnapBackfillState::Running(_)));
        assert_eq!(
            SnapStateStore::new(&backfill.provider_factory).interrupted_generation().unwrap(),
            Some(generation)
        );
    }

    #[test]
    fn invalid_generation_does_not_start_either_sync_path() {
        let mut backfill = backfill();
        let provider = backfill.provider_factory.database_provider_rw().unwrap();
        provider.save_stage_checkpoint_progress(StageId::Other("SnapSync"), vec![0xff]).unwrap();
        provider.commit().unwrap();

        backfill.on_action(BackfillAction::Start(PipelineTarget::Sync(B256::repeat_byte(1))));
        assert!(matches!(
            backfill.poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(BackfillEvent::Finished(Err(_)))
        ));
        assert!(matches!(backfill.state, SnapBackfillState::Idle(Some(_))));
    }
}
