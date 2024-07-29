use crate::backfill::{BackfillAction, BackfillEvent, BackfillSync};
use futures::Stream;
use reth_stages_api::PipelineTarget;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// The type that drives the chain forward.
///
/// A state machine that orchestrates the components responsible for advancing the chain
///
///
/// ## Control flow
///
/// The [`ChainOrchestrator`] is responsible for controlling the backfill sync and additional hooks.
/// It polls the given `handler`, which is responsible for advancing the chain, how is up to the
/// handler. However, due to database restrictions (e.g. exclusive write access), following
/// invariants apply:
///  - If the handler requests a backfill run (e.g. [`BackfillAction::Start`]), the handler must
///    ensure that while the backfill sync is running, no other write access is granted.
///  - At any time the [`ChainOrchestrator`] can request exclusive write access to the database
///    (e.g. if pruning is required), but will not do so until the handler has acknowledged the
///    request for write access.
///
/// The [`ChainOrchestrator`] polls the [`ChainHandler`] to advance the chain and handles the
/// emitted events. Requests and events are passed to the [`ChainHandler`] via
/// [`ChainHandler::on_event`].
#[must_use = "Stream does nothing unless polled"]
#[derive(Debug)]
pub struct ChainOrchestrator<T, P>
where
    T: ChainHandler,
    P: BackfillSync,
{
    /// The handler for advancing the chain.
    handler: T,
    /// Controls backfill sync.
    backfill_sync: P,
}

impl<T, P> ChainOrchestrator<T, P>
where
    T: ChainHandler + Unpin,
    P: BackfillSync + Unpin,
{
    /// Creates a new [`ChainOrchestrator`] with the given handler and backfill sync.
    pub const fn new(handler: T, backfill_sync: P) -> Self {
        Self { handler, backfill_sync }
    }

    /// Returns the handler
    pub const fn handler(&self) -> &T {
        &self.handler
    }

    /// Returns a mutable reference to the handler
    pub fn handler_mut(&mut self) -> &mut T {
        &mut self.handler
    }

    /// Triggers a backfill sync for the __valid__ given target.
    ///
    /// CAUTION: This function should be used with care and with a valid target.
    pub fn start_backfill_sync(&mut self, target: impl Into<PipelineTarget>) {
        self.backfill_sync.on_action(BackfillAction::Start(target.into()));
    }

    /// Internal function used to advance the chain.
    ///
    /// Polls the `ChainOrchestrator` for the next event.
    #[tracing::instrument(level = "debug", name = "ChainOrchestrator::poll", skip(self, cx))]
    fn poll_next_event(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ChainEvent<T::Event>> {
        let this = self.get_mut();

        // This loop polls the components
        //
        // 1. Polls the backfill sync to completion, if active.
        // 2. Advances the chain by polling the handler.
        'outer: loop {
            // try to poll the backfill sync to completion, if active
            match this.backfill_sync.poll(cx) {
                Poll::Ready(backfill_sync_event) => match backfill_sync_event {
                    BackfillEvent::Idle => {}
                    BackfillEvent::Started(_) => {
                        // notify handler that backfill sync started
                        this.handler.on_event(FromOrchestrator::BackfillSyncStarted);
                        return Poll::Ready(ChainEvent::BackfillSyncStarted);
                    }
                    BackfillEvent::Finished(res) => {
                        return match res {
                            Ok(event) => {
                                tracing::debug!(?event, "backfill sync finished");
                                // notify handler that backfill sync finished
                                this.handler.on_event(FromOrchestrator::BackfillSyncFinished);
                                Poll::Ready(ChainEvent::BackfillSyncFinished)
                            }
                            Err(err) => {
                                tracing::error!( %err, "backfill sync failed");
                                Poll::Ready(ChainEvent::FatalError)
                            }
                        }
                    }
                    BackfillEvent::TaskDropped(err) => {
                        tracing::error!( %err, "backfill sync task dropped");
                        return Poll::Ready(ChainEvent::FatalError);
                    }
                },
                Poll::Pending => {}
            }

            // poll the handler for the next event
            match this.handler.poll(cx) {
                Poll::Ready(handler_event) => {
                    match handler_event {
                        HandlerEvent::BackfillSync(target) => {
                            // trigger backfill sync and start polling it
                            this.backfill_sync.on_action(BackfillAction::Start(target));
                            continue 'outer
                        }
                        HandlerEvent::Event(ev) => {
                            // bubble up the event
                            return Poll::Ready(ChainEvent::Handler(ev));
                        }
                    }
                }
                Poll::Pending => {
                    // no more events to process
                    break 'outer
                }
            }
        }

        Poll::Pending
    }
}

impl<T, P> Stream for ChainOrchestrator<T, P>
where
    T: ChainHandler + Unpin,
    P: BackfillSync + Unpin,
{
    type Item = ChainEvent<T::Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.as_mut().poll_next_event(cx).map(Some)
    }
}

/// Represents the sync mode the chain is operating in.
#[derive(Debug, Default)]
enum SyncMode {
    #[default]
    Handler,
    Backfill,
}

/// Event emitted by the [`ChainOrchestrator`]
///
/// These are meant to be used for observability and debugging purposes.
#[derive(Debug)]
pub enum ChainEvent<T> {
    /// Backfill sync started
    BackfillSyncStarted,
    /// Backfill sync finished
    BackfillSyncFinished,
    /// Fatal error
    FatalError,
    /// Event emitted by the handler
    Handler(T),
}

/// A trait that advances the chain by handling actions.
///
/// This is intended to be implement the chain consensus logic, for example `engine` API.
pub trait ChainHandler: Send + Sync {
    /// Event generated by this handler that orchestrator can bubble up;
    type Event: Send;

    /// Informs the handler about an event from the [`ChainOrchestrator`].
    fn on_event(&mut self, event: FromOrchestrator);

    /// Polls for actions that [`ChainOrchestrator`] should handle.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<HandlerEvent<Self::Event>>;
}

/// Events/Requests that the [`ChainHandler`] can emit to the [`ChainOrchestrator`].
#[derive(Clone, Debug)]
pub enum HandlerEvent<T> {
    /// Request to start a backfill sync
    BackfillSync(PipelineTarget),
    /// Other event emitted by the handler
    Event(T),
}

/// Internal events issued by the [`ChainOrchestrator`].
#[derive(Clone, Debug)]
pub enum FromOrchestrator {
    /// Invoked when backfill sync finished
    BackfillSyncFinished,
    /// Invoked when backfill sync started
    BackfillSyncStarted,
}

/// Represents the state of the chain.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum OrchestratorState {
    /// Orchestrator has exclusive write access to the database.
    BackfillSyncActive,
    /// Node is actively processing the chain.
    #[default]
    Idle,
}

impl OrchestratorState {
    /// Returns `true` if the state is [`OrchestratorState::BackfillSyncActive`].
    pub const fn is_backfill_sync_active(&self) -> bool {
        matches!(self, Self::BackfillSyncActive)
    }

    /// Returns `true` if the state is [`OrchestratorState::Idle`].
    pub const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
}
