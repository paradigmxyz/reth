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
/// The [`ChainOrchestrator`] is responsible for controlling the pipeline sync and additional hooks.
/// It polls the given `handler`, which is responsible for advancing the chain, how is up to the
/// handler. However, due to database restrictions (e.g. exclusive write access), following
/// invariants apply:
///  - If the handler requests a backfill run (e.g. [`BackfillAction::Start`]), the handler must
///    ensure that while the pipeline is running, no other write access is granted.
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
    /// Controls pipeline sync.
    pipeline: P,
}

impl<T, P> ChainOrchestrator<T, P>
where
    T: ChainHandler + Unpin,
    P: BackfillSync + Unpin,
{
    /// Creates a new [`ChainOrchestrator`] with the given handler and pipeline.
    pub const fn new(handler: T, pipeline: P) -> Self {
        Self { handler, pipeline }
    }

    /// Returns the handler
    pub const fn handler(&self) -> &T {
        &self.handler
    }

    /// Returns a mutable reference to the handler
    pub fn handler_mut(&mut self) -> &mut T {
        &mut self.handler
    }

    /// Internal function used to advance the chain.
    ///
    /// Polls the `ChainOrchestrator` for the next event.
    #[tracing::instrument(level = "debug", name = "ChainOrchestrator::poll", skip(self, cx))]
    fn poll_next_event(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ChainEvent<T::Event>> {
        let this = self.get_mut();

        // This loop polls the components
        //
        // 1. Polls the pipeline to completion, if active.
        // 2. Advances the chain by polling the handler.
        'outer: loop {
            // try to poll the pipeline to completion, if active
            match this.pipeline.poll(cx) {
                Poll::Ready(pipeline_event) => match pipeline_event {
                    BackfillEvent::Idle => {}
                    BackfillEvent::Started(_) => {
                        // notify handler that pipeline started
                        this.handler.on_event(FromOrchestrator::PipelineStarted);
                        return Poll::Ready(ChainEvent::PipelineStarted);
                    }
                    BackfillEvent::Finished(res) => {
                        return match res {
                            Ok(event) => {
                                tracing::debug!(?event, "pipeline finished");
                                // notify handler that pipeline finished
                                this.handler.on_event(FromOrchestrator::PipelineFinished);
                                Poll::Ready(ChainEvent::PipelineFinished)
                            }
                            Err(err) => {
                                tracing::error!( %err, "pipeline failed");
                                Poll::Ready(ChainEvent::FatalError)
                            }
                        }
                    }
                    BackfillEvent::TaskDropped(err) => {
                        tracing::error!( %err, "pipeline task dropped");
                        return Poll::Ready(ChainEvent::FatalError);
                    }
                },
                Poll::Pending => {}
            }

            // poll the handler for the next event
            match this.handler.poll(cx) {
                Poll::Ready(handler_event) => {
                    match handler_event {
                        HandlerEvent::Pipeline(target) => {
                            // trigger pipeline and start polling it
                            this.pipeline.on_action(BackfillAction::Start(target));
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
    /// Pipeline sync started
    PipelineStarted,
    /// Pipeline sync finished
    PipelineFinished,
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
    /// Request to start a pipeline sync
    Pipeline(PipelineTarget),
    /// Other event emitted by the handler
    Event(T),
}

/// Internal events issued by the [`ChainOrchestrator`].
#[derive(Clone, Debug)]
pub enum FromOrchestrator {
    /// Invoked when pipeline sync finished
    PipelineFinished,
    /// Invoked when pipeline started
    PipelineStarted,
}

/// Represents the state of the chain.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum OrchestratorState {
    /// Orchestrator has exclusive write access to the database.
    PipelineActive,
    /// Node is actively processing the chain.
    #[default]
    Idle,
}

impl OrchestratorState {
    /// Returns `true` if the state is [`OrchestratorState::PipelineActive`].
    pub const fn is_pipeline_active(&self) -> bool {
        matches!(self, Self::PipelineActive)
    }

    /// Returns `true` if the state is [`OrchestratorState::Idle`].
    pub const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
}
