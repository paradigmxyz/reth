//! Prune hook for the engine implementation.

use crate::{
    engine::hooks::{EngineContext, EngineHook, EngineHookError, EngineHookEvent},
    hooks::EngineHookDBAccessLevel,
};
use futures::FutureExt;
use metrics::Counter;
use reth_db::database::Database;
use reth_interfaces::{RethError, RethResult};
use reth_primitives::BlockNumber;
use reth_prune::{Pruner, PrunerError, PrunerWithResult};
use reth_tasks::TaskSpawner;
use std::{
    fmt,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;

/// Manages pruning under the control of the engine.
///
/// This type controls the [Pruner].
pub struct PruneHook<DB> {
    /// The current state of the pruner.
    pruner_state: PrunerState<DB>,
    /// The type that can spawn the pruner task.
    pruner_task_spawner: Box<dyn TaskSpawner>,
    metrics: Metrics,
}

impl<DB: fmt::Debug> fmt::Debug for PruneHook<DB> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PruneHook")
            .field("pruner_state", &self.pruner_state)
            .field("metrics", &self.metrics)
            .finish()
    }
}

impl<DB: Database + 'static> PruneHook<DB> {
    /// Create a new instance
    pub fn new(pruner: Pruner<DB>, pruner_task_spawner: Box<dyn TaskSpawner>) -> Self {
        Self {
            pruner_state: PrunerState::Idle(Some(pruner)),
            pruner_task_spawner,
            metrics: Metrics::default(),
        }
    }

    /// Advances the pruner state.
    ///
    /// This checks for the result in the channel, or returns pending if the pruner is idle.
    fn poll_pruner(&mut self, cx: &mut Context<'_>) -> Poll<RethResult<EngineHookEvent>> {
        let result = match self.pruner_state {
            PrunerState::Idle(_) => return Poll::Pending,
            PrunerState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };

        let event = match result {
            Ok((pruner, result)) => {
                self.pruner_state = PrunerState::Idle(Some(pruner));

                match result {
                    Ok(_) => EngineHookEvent::Finished(Ok(())),
                    Err(err) => EngineHookEvent::Finished(Err(err.into())),
                }
            }
            Err(_) => {
                // failed to receive the pruner
                EngineHookEvent::Finished(Err(EngineHookError::ChannelClosed))
            }
        };

        Poll::Ready(Ok(event))
    }

    /// This will try to spawn the pruner if it is idle:
    /// 1. Check if pruning is needed through [Pruner::is_pruning_needed].
    /// 2.
    ///     1. If pruning is needed, pass tip block number to the [Pruner::run] and spawn it in a
    /// separate task. Set pruner state to [PrunerState::Running].
    ///     2. If pruning is not needed, set pruner state back to [PrunerState::Idle].
    ///
    /// If pruner is already running, do nothing.
    fn try_spawn_pruner(&mut self, tip_block_number: BlockNumber) -> Option<EngineHookEvent> {
        match &mut self.pruner_state {
            PrunerState::Idle(pruner) => {
                let mut pruner = pruner.take()?;

                // Check tip for pruning
                if pruner.is_pruning_needed(tip_block_number) {
                    let (tx, rx) = oneshot::channel();
                    self.pruner_task_spawner.spawn_critical_blocking(
                        "pruner task",
                        Box::pin(async move {
                            let result = pruner.run(tip_block_number);
                            let _ = tx.send((pruner, result));
                        }),
                    );
                    self.metrics.runs.increment(1);
                    self.pruner_state = PrunerState::Running(rx);

                    Some(EngineHookEvent::Started)
                } else {
                    self.pruner_state = PrunerState::Idle(Some(pruner));
                    Some(EngineHookEvent::NotReady)
                }
            }
            PrunerState::Running(_) => None,
        }
    }
}

impl<DB: Database + 'static> EngineHook for PruneHook<DB> {
    fn name(&self) -> &'static str {
        "Prune"
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        ctx: EngineContext,
    ) -> Poll<RethResult<EngineHookEvent>> {
        // Try to spawn a pruner
        match self.try_spawn_pruner(ctx.tip_block_number) {
            Some(EngineHookEvent::NotReady) => return Poll::Pending,
            Some(event) => return Poll::Ready(Ok(event)),
            None => (),
        }

        // Poll pruner and check its status
        self.poll_pruner(cx)
    }

    fn db_access_level(&self) -> EngineHookDBAccessLevel {
        EngineHookDBAccessLevel::ReadWrite
    }
}

/// The possible pruner states within the sync controller.
///
/// [PrunerState::Idle] means that the pruner is currently idle.
/// [PrunerState::Running] means that the pruner is currently running.
///
/// NOTE: The differentiation between these two states is important, because when the pruner is
/// running, it acquires the write lock over the database. This means that we cannot forward to the
/// blockchain tree any messages that would result in database writes, since it would result in a
/// deadlock.
#[derive(Debug)]
enum PrunerState<DB> {
    /// Pruner is idle.
    Idle(Option<Pruner<DB>>),
    /// Pruner is running and waiting for a response
    Running(oneshot::Receiver<PrunerWithResult<DB>>),
}

#[derive(reth_metrics::Metrics)]
#[metrics(scope = "consensus.engine.prune")]
struct Metrics {
    /// The number of times the pruner was run.
    runs: Counter,
}

impl From<PrunerError> for EngineHookError {
    fn from(err: PrunerError) -> Self {
        match err {
            PrunerError::PruneSegment(_) | PrunerError::InconsistentData(_) => {
                EngineHookError::Internal(Box::new(err))
            }
            PrunerError::Interface(err) => err.into(),
            PrunerError::Database(err) => RethError::Database(err).into(),
            PrunerError::Provider(err) => RethError::Provider(err).into(),
        }
    }
}
