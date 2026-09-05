//! Snapshot bootstrap wired into the engine's backfill slot.
//!
//! Snap assembles state from peers instead of executing history, but it still needs canonical
//! headers first and the ordinary stages afterwards. Running it as a [`BackfillSync`] gives that
//! sequence the same exclusive database access the staged pipeline gets, so the engine buffers
//! payloads for the whole bootstrap rather than racing it.

use futures::FutureExt;
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync};
use reth_errors::RethError;
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_snap_sync::{NodeSnapContext, SnapPivotPolicy, SnapSyncOutcome, SnapSyncSession};
use reth_stages_api::{ControlFlow, Pipeline, PipelineError, PipelineTarget, StageId};
use reth_tasks::{shutdown::signal, Runtime};
use std::task::{ready, Context, Poll};
use tokio::sync::oneshot;
use tracing::{debug, info};

/// Drives a snapshot bootstrap through the engine's backfill interface.
///
/// One backfill run is headers, then state download, then the remaining stages above the published
/// pivot. [`BackfillEvent::Finished`] is only emitted once all three are done, so the engine never
/// treats the persisted chain as ready while stage bookkeeping is still inconsistent.
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
            policy: SnapPivotPolicy::new(),
            state: SnapBackfillState::Idle(Some(Box::new(pipeline))),
            pending_target: None,
        }
    }

    /// Sets the pivot distance and history bounds the bootstrap enforces.
    pub const fn with_policy(mut self, policy: SnapPivotPolicy) -> Self {
        self.policy = policy;
        self
    }

    const fn is_idle(&self) -> bool {
        matches!(self.state, SnapBackfillState::Idle(_))
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
        let pipeline = pipeline.take().expect("idle backfill owns its pipeline");

        let (tx, rx) = oneshot::channel();
        let client = self.client.clone();
        let provider_factory = self.provider_factory.clone();
        let runtime = self.task_spawner.clone();
        let policy = self.policy;

        self.task_spawner.spawn_critical_blocking_task("snap backfill task", async move {
            let result =
                bootstrap(pipeline, client, provider_factory, runtime, policy, target).await;
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
                self.state = SnapBackfillState::Idle(Some(pipeline));
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
        match action {
            BackfillAction::Start(target) => self.set_target(target),
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        if let Some(event) = self.try_spawn() {
            return Poll::Ready(event)
        }
        if !self.is_idle() {
            return self.poll_bootstrap(cx)
        }
        Poll::Pending
    }
}

/// What one bootstrap hands back: the pipeline it borrowed, and how its final run ended.
type BootstrapResult<N> = (Box<Pipeline<N>>, Result<ControlFlow, PipelineError>);

/// Owns the pipeline while idle and the bootstrap's result channel while running.
///
/// The distinction matters for the same reason it does for the staged pipeline: a running
/// bootstrap holds the database write lock, so no other component may write while it is active.
#[derive(Debug)]
enum SnapBackfillState<N: ProviderNodeTypes> {
    /// No bootstrap in flight; the pipeline is parked here.
    Idle(Option<Box<Pipeline<N>>>),
    /// A bootstrap is running and will return the pipeline with its result.
    Running(oneshot::Receiver<BootstrapResult<N>>),
}

/// Runs headers, then the state download, then the stages above the published pivot.
async fn bootstrap<N, C>(
    mut pipeline: Box<Pipeline<N>>,
    client: C,
    provider_factory: ProviderFactory<N>,
    runtime: Runtime,
    policy: SnapPivotPolicy,
    target: PipelineTarget,
) -> BootstrapResult<N>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient,
{
    // Snap needs canonical headers and their BAL commitments, but nothing below the pivot may be
    // executed, so only the header prefix of the pipeline runs first.
    if let Err(error) = pipeline.run_until(StageId::Headers, Some(target)).await {
        return (pipeline, Err(error))
    }

    // The session borrows locals, so the bootstrap owns the clones it was handed.
    let (_signal, shutdown) = signal();
    let context = NodeSnapContext::new(&provider_factory, &client, shutdown);
    let outcome = SnapSyncSession::new(&client, &provider_factory, context, runtime)
        .with_policy(policy)
        .run()
        .await;

    match outcome {
        Ok(SnapSyncOutcome::Complete { generation }) => {
            info!(
                target: "sync::snap",
                block_number = generation.target_block,
                "Snap state published, resuming the pipeline above the pivot"
            );
        }
        Ok(SnapSyncOutcome::Stalled { generation }) => {
            // A stalled bootstrap leaves its generation resumable, so the remaining stages still
            // run and the next backfill request picks the download back up.
            debug!(
                target: "sync::snap",
                resumable = generation.is_some(),
                "Snap state download made no further progress"
            );
        }
        Err(error) => return (pipeline, Err(PipelineError::Internal(RethError::other(error)))),
    }

    // Stages the published frontier satisfies skip straight to the pivot, so this only executes
    // what is genuinely missing above it.
    let (pipeline, result) = (*pipeline).run_as_fut(Some(target)).await;
    (Box::new(pipeline), result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_network_p2p::NoopFullBlockClient;
    use reth_provider::test_utils::{create_test_provider_factory, MockNodeTypesWithDB};
    use reth_prune::PruneModes;
    use reth_static_file::StaticFileProducer;
    use std::task::Waker;

    fn backfill() -> SnapBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient> {
        let factory = create_test_provider_factory();
        let pipeline = Pipeline::<MockNodeTypesWithDB>::builder().build(
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
        assert!(!backfill.is_idle());
    }
}
