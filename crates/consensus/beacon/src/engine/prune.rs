//! Prune management for the engine implementation.

use futures::FutureExt;
use reth_primitives::BlockNumber;
use reth_prune::{Pruner, PrunerError, PrunerWithResult};
use reth_tasks::TaskSpawner;
use std::task::{ready, Context, Poll};
use tokio::sync::oneshot;

/// Manages pruning under the control of the engine.
///
/// This type controls the [Pruner].
pub(crate) struct EnginePruneController {
    /// The current state of the pruner.
    pruner_state: PrunerState,
    /// The type that can spawn the pruner task.
    pruner_task_spawner: Box<dyn TaskSpawner>,
}

impl EnginePruneController {
    /// Create a new instance
    pub(crate) fn new(pruner: Pruner, pruner_task_spawner: Box<dyn TaskSpawner>) -> Self {
        Self { pruner_state: PrunerState::Idle(Some(pruner)), pruner_task_spawner }
    }

    /// Returns `true` if the pruner is idle.
    pub(crate) fn is_pruner_idle(&self) -> bool {
        self.pruner_state.is_idle()
    }

    /// Advances the pruner state.
    ///
    /// This checks for the result in the channel, or returns pending if the pruner is idle.
    fn poll_pruner(&mut self, cx: &mut Context<'_>) -> Poll<EnginePruneEvent> {
        let res = match self.pruner_state {
            PrunerState::Idle(_) => return Poll::Pending,
            PrunerState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };
        let ev = match res {
            Ok((pruner, result)) => {
                self.pruner_state = PrunerState::Idle(Some(pruner));
                EnginePruneEvent::Finished { result }
            }
            Err(_) => {
                // failed to receive the pruner
                EnginePruneEvent::TaskDropped
            }
        };
        Poll::Ready(ev)
    }

    /// This will try to spawn the pruner if it is idle:
    /// 1. Check if pruning is needed through [Pruner::is_pruning_needed].
    /// 2a. If pruning is needed, pass tip block number to the [Pruner::run] and spawn it in a
    /// separate task. Set pruner state to [PrunerState::Running].
    /// 2b. If pruning is not needed, set pruner state back to [PrunerState::Idle].
    ///
    /// If pruner is already running, do nothing.
    fn try_spawn_pruner(&mut self, tip_block_number: BlockNumber) -> Option<EnginePruneEvent> {
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
                    self.pruner_state = PrunerState::Running(rx);

                    Some(EnginePruneEvent::Started(tip_block_number))
                } else {
                    self.pruner_state = PrunerState::Idle(Some(pruner));
                    Some(EnginePruneEvent::NotReady)
                }
            }
            PrunerState::Running(_) => None,
        }
    }

    /// Advances the prune process with the tip block number.
    pub(crate) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        tip_block_number: BlockNumber,
    ) -> Poll<EnginePruneEvent> {
        // Try to spawn a pruner
        match self.try_spawn_pruner(tip_block_number) {
            Some(EnginePruneEvent::NotReady) => return Poll::Pending,
            Some(event) => return Poll::Ready(event),
            None => (),
        }

        // Poll pruner and check its status
        self.poll_pruner(cx)
    }
}

/// The event type emitted by the [EnginePruneController].
#[derive(Debug)]
pub(crate) enum EnginePruneEvent {
    /// Pruner is not ready
    NotReady,
    /// Pruner started with tip block number
    Started(BlockNumber),
    /// Pruner finished
    ///
    /// If this is returned, the pruner is idle.
    Finished {
        /// Final result of the pruner run.
        result: Result<(), PrunerError>,
    },
    /// Pruner task was dropped after it was started, unable to receive it because channel
    /// closed. This would indicate a panicked pruner task
    TaskDropped,
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
enum PrunerState {
    /// Pruner is idle.
    Idle(Option<Pruner>),
    /// Pruner is running and waiting for a response
    Running(oneshot::Receiver<PrunerWithResult>),
}

impl PrunerState {
    /// Returns `true` if the state matches idle.
    fn is_idle(&self) -> bool {
        matches!(self, PrunerState::Idle(_))
    }
}
