//! `StaticFile` hook for the engine implementation.

use crate::{
    engine::hooks::{EngineHook, EngineHookContext, EngineHookError, EngineHookEvent},
    hooks::EngineHookDBAccessLevel,
};
use alloy_primitives::BlockNumber;
use futures::FutureExt;
use reth_errors::RethResult;
use reth_node_types::NodeTypesWithDB;
use reth_primitives::static_file::HighestStaticFiles;
use reth_provider::providers::ProviderNodeTypes;
use reth_static_file::{StaticFileProducer, StaticFileProducerWithResult};
use reth_tasks::TaskSpawner;
use std::task::{ready, Context, Poll};
use tokio::sync::oneshot;
use tracing::trace;

/// Manages producing static files under the control of the engine.
///
/// This type controls the [`StaticFileProducer`].
#[derive(Debug)]
pub struct StaticFileHook<N: NodeTypesWithDB> {
    /// The current state of the `static_file_producer`.
    state: StaticFileProducerState<N>,
    /// The type that can spawn the `static_file_producer` task.
    task_spawner: Box<dyn TaskSpawner>,
}

impl<N: ProviderNodeTypes> StaticFileHook<N> {
    /// Create a new instance
    pub fn new(
        static_file_producer: StaticFileProducer<N>,
        task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        Self { state: StaticFileProducerState::Idle(Some(static_file_producer)), task_spawner }
    }

    /// Advances the `static_file_producer` state.
    ///
    /// This checks for the result in the channel, or returns pending if the `static_file_producer`
    /// is idle.
    fn poll_static_file_producer(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<RethResult<EngineHookEvent>> {
        let result = match self.state {
            StaticFileProducerState::Idle(_) => return Poll::Pending,
            StaticFileProducerState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };

        let event = match result {
            Ok((static_file_producer, result)) => {
                self.state = StaticFileProducerState::Idle(Some(static_file_producer));

                match result {
                    Ok(_) => EngineHookEvent::Finished(Ok(())),
                    Err(err) => EngineHookEvent::Finished(Err(EngineHookError::Common(err.into()))),
                }
            }
            Err(_) => {
                // failed to receive the static_file_producer
                EngineHookEvent::Finished(Err(EngineHookError::ChannelClosed))
            }
        };

        Poll::Ready(Ok(event))
    }

    /// This will try to spawn the `static_file_producer` if it is idle:
    /// 1. Check if producing static files is needed through
    ///    [`StaticFileProducer::get_static_file_targets`](reth_static_file::StaticFileProducerInner::get_static_file_targets)
    ///    and then [`StaticFileTargets::any`](reth_static_file::StaticFileTargets::any).
    ///
    /// 2.1. If producing static files is needed, pass static file request to the
    ///      [`StaticFileProducer::run`](reth_static_file::StaticFileProducerInner::run) and
    ///      spawn it in a separate task. Set static file producer state to
    ///      [`StaticFileProducerState::Running`].
    /// 2.2. If producing static files is not needed, set static file producer state back to
    ///      [`StaticFileProducerState::Idle`].
    ///
    /// If `static_file_producer` is already running, do nothing.
    fn try_spawn_static_file_producer(
        &mut self,
        finalized_block_number: BlockNumber,
    ) -> RethResult<Option<EngineHookEvent>> {
        Ok(match &mut self.state {
            StaticFileProducerState::Idle(static_file_producer) => {
                let Some(static_file_producer) = static_file_producer.take() else {
                    trace!(target: "consensus::engine::hooks::static_file", "StaticFileProducer is already running but the state is idle");
                    return Ok(None)
                };

                let Some(locked_static_file_producer) = static_file_producer.try_lock_arc() else {
                    trace!(target: "consensus::engine::hooks::static_file", "StaticFileProducer lock is already taken");
                    return Ok(None)
                };

                let targets =
                    locked_static_file_producer.get_static_file_targets(HighestStaticFiles {
                        headers: Some(finalized_block_number),
                        receipts: Some(finalized_block_number),
                        transactions: Some(finalized_block_number),
                    })?;

                // Check if the moving data to static files has been requested.
                if targets.any() {
                    let (tx, rx) = oneshot::channel();
                    self.task_spawner.spawn_critical_blocking(
                        "static_file_producer task",
                        Box::pin(async move {
                            let result = locked_static_file_producer.run(targets);
                            let _ = tx.send((static_file_producer, result));
                        }),
                    );
                    self.state = StaticFileProducerState::Running(rx);

                    Some(EngineHookEvent::Started)
                } else {
                    self.state = StaticFileProducerState::Idle(Some(static_file_producer));
                    Some(EngineHookEvent::NotReady)
                }
            }
            StaticFileProducerState::Running(_) => None,
        })
    }
}

impl<N: ProviderNodeTypes> EngineHook for StaticFileHook<N> {
    fn name(&self) -> &'static str {
        "StaticFile"
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        ctx: EngineHookContext,
    ) -> Poll<RethResult<EngineHookEvent>> {
        let Some(finalized_block_number) = ctx.finalized_block_number else {
            trace!(target: "consensus::engine::hooks::static_file", ?ctx, "Finalized block number is not available");
            return Poll::Pending
        };

        // Try to spawn a static_file_producer
        match self.try_spawn_static_file_producer(finalized_block_number)? {
            Some(EngineHookEvent::NotReady) => return Poll::Pending,
            Some(event) => return Poll::Ready(Ok(event)),
            None => (),
        }

        // Poll static_file_producer and check its status
        self.poll_static_file_producer(cx)
    }

    fn db_access_level(&self) -> EngineHookDBAccessLevel {
        EngineHookDBAccessLevel::ReadOnly
    }
}

/// The possible `static_file_producer` states within the sync controller.
///
/// [`StaticFileProducerState::Idle`] means that the static file producer is currently idle.
/// [`StaticFileProducerState::Running`] means that the static file producer is currently running.
#[derive(Debug)]
enum StaticFileProducerState<N: NodeTypesWithDB> {
    /// [`StaticFileProducer`] is idle.
    Idle(Option<StaticFileProducer<N>>),
    /// [`StaticFileProducer`] is running and waiting for a response
    Running(oneshot::Receiver<StaticFileProducerWithResult<N>>),
}
