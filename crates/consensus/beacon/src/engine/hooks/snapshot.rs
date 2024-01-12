//! Snapshot hook for the engine implementation.

use crate::{
    engine::hooks::{EngineContext, EngineHook, EngineHookError, EngineHookEvent},
    hooks::EngineHookDBAccessLevel,
};
use futures::FutureExt;
use reth_db::database::Database;
use reth_interfaces::{RethError, RethResult};
use reth_primitives::BlockNumber;
use reth_snapshot::{Snapshotter, SnapshotterError, SnapshotterWithResult};
use reth_tasks::TaskSpawner;
use std::task::{ready, Context, Poll};
use tokio::sync::oneshot;

/// Manages snapshotting under the control of the engine.
///
/// This type controls the [Snapshotter].
#[derive(Debug)]
pub struct SnapshotHook<DB> {
    /// The current state of the snapshotter.
    state: SnapshotterState<DB>,
    /// The type that can spawn the snapshotter task.
    task_spawner: Box<dyn TaskSpawner>,
}

impl<DB: Database + 'static> SnapshotHook<DB> {
    /// Create a new instance
    pub fn new(snapshotter: Snapshotter<DB>, task_spawner: Box<dyn TaskSpawner>) -> Self {
        Self { state: SnapshotterState::Idle(Some(snapshotter)), task_spawner }
    }

    /// Advances the snapshotter state.
    ///
    /// This checks for the result in the channel, or returns pending if the snapshotter is idle.
    fn poll_snapshotter(&mut self, cx: &mut Context<'_>) -> Poll<RethResult<EngineHookEvent>> {
        let result = match self.state {
            SnapshotterState::Idle(_) => return Poll::Pending,
            SnapshotterState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };

        let event = match result {
            Ok((snapshotter, result)) => {
                self.state = SnapshotterState::Idle(Some(snapshotter));

                match result {
                    Ok(_) => EngineHookEvent::Finished(Ok(())),
                    Err(err) => EngineHookEvent::Finished(Err(err.into())),
                }
            }
            Err(_) => {
                // failed to receive the snapshotter
                EngineHookEvent::Finished(Err(EngineHookError::ChannelClosed))
            }
        };

        Poll::Ready(Ok(event))
    }

    /// This will try to spawn the snapshotter if it is idle:
    /// 1. Check if snapshotting is needed through [Snapshotter::get_snapshot_targets] and then
    ///    [SnapshotTargets::any](reth_snapshot::SnapshotTargets::any).
    /// 2.
    ///     1. If snapshotting is needed, pass snapshot request to the [Snapshotter::run] and spawn
    ///        it in a separate task. Set snapshotter state to [SnapshotterState::Running].
    ///     2. If snapshotting is not needed, set snapshotter state back to
    ///        [SnapshotterState::Idle].
    ///
    /// If snapshotter is already running, do nothing.
    fn try_spawn_snapshotter(
        &mut self,
        finalized_block_number: BlockNumber,
    ) -> RethResult<Option<EngineHookEvent>> {
        Ok(match &mut self.state {
            SnapshotterState::Idle(snapshotter) => {
                let Some(mut snapshotter) = snapshotter.take() else { return Ok(None) };

                let targets = snapshotter.get_snapshot_targets(finalized_block_number)?;

                // Check if the snapshotting of any data has been requested.
                if targets.any() {
                    let (tx, rx) = oneshot::channel();
                    self.task_spawner.spawn_critical_blocking(
                        "snapshotter task",
                        Box::pin(async move {
                            let result = snapshotter.run(targets);
                            let _ = tx.send((snapshotter, result));
                        }),
                    );
                    self.state = SnapshotterState::Running(rx);

                    Some(EngineHookEvent::Started)
                } else {
                    self.state = SnapshotterState::Idle(Some(snapshotter));
                    Some(EngineHookEvent::NotReady)
                }
            }
            SnapshotterState::Running(_) => None,
        })
    }
}

impl<DB: Database + 'static> EngineHook for SnapshotHook<DB> {
    fn name(&self) -> &'static str {
        "Snapshot"
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        ctx: EngineContext,
    ) -> Poll<RethResult<EngineHookEvent>> {
        let Some(finalized_block_number) = ctx.finalized_block_number else {
            return Poll::Ready(Ok(EngineHookEvent::NotReady))
        };

        // Try to spawn a snapshotter
        match self.try_spawn_snapshotter(finalized_block_number)? {
            Some(EngineHookEvent::NotReady) => return Poll::Pending,
            Some(event) => return Poll::Ready(Ok(event)),
            None => (),
        }

        // Poll snapshotter and check its status
        self.poll_snapshotter(cx)
    }

    fn db_access_level(&self) -> EngineHookDBAccessLevel {
        EngineHookDBAccessLevel::ReadOnly
    }
}

/// The possible snapshotter states within the sync controller.
///
/// [SnapshotterState::Idle] means that the snapshotter is currently idle.
/// [SnapshotterState::Running] means that the snapshotter is currently running.
#[derive(Debug)]
enum SnapshotterState<DB> {
    /// Snapshotter is idle.
    Idle(Option<Snapshotter<DB>>),
    /// Snapshotter is running and waiting for a response
    Running(oneshot::Receiver<SnapshotterWithResult<DB>>),
}

impl From<SnapshotterError> for EngineHookError {
    fn from(err: SnapshotterError) -> Self {
        match err {
            SnapshotterError::InconsistentData(_) => EngineHookError::Internal(Box::new(err)),
            SnapshotterError::Interface(err) => err.into(),
            SnapshotterError::Database(err) => RethError::Database(err).into(),
            SnapshotterError::Provider(err) => RethError::Provider(err).into(),
        }
    }
}
