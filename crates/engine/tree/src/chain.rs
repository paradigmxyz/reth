use futures::Stream;
use reth_primitives::B256;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// The type that drives the chain forward.
///
/// A state machine that orchestrates the components responsible for advancing the chain
// Reacts to custom requests
#[must_use = "Stream does nothing unless polled"]
pub struct ChainOrchestrator<T>
where
    T: ChainHandler,
{
    /// The handler responsible for advancing the chain.
    handler: T,
    /* pipeline */

    /* pruning */

    sync: (),
}

impl<T> ChainOrchestrator<T>
where
    T: ChainHandler,
{
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
    fn poll_next_event(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ChainEvent> {
        todo!()
    }
}

impl<T> Stream for ChainOrchestrator<T>
where
    T: ChainHandler,
{
    type Item = ChainEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.as_mut().poll_next_event(cx).map(Some)
    }
}

/// Event emitted by the [`ChainOrchestrator`]
pub enum ChainEvent {
    /// Synced new head.
    Synced(B256),
}

/// Events/Requests that the [`ChainHandler`] can emit to the [`ChainOrchestrator`].
#[derive(Debug, Clone)]
pub enum HandlerEvent {
    /// Instructs the Swarm
    Synced(B256),
    /// Request to sync to a specific target
    SyncRequest(B256),
}

/// Internal events issued by the [`ChainOrchestrator`].
pub enum FromOrchestrator {
    /// Started syncing or pruning which requires mutable access to the db.
    Pausing,
    /// Finished syncing or pruning
    ///
    /// TODO: pipeline finished, pruner finished
    Active,
}

/// A trait that advances the chain by handling actions.
///
/// This is intended to be implement the chain consensus logic, for example `engine` API.
pub trait ChainHandler: Send + Sync {
    /// Informs the handler about an event from the [`ChainOrchestrator`].
    fn on_event(&mut self, event: FromOrchestrator);

    /// Polls for actions that [`ChainOrchestrator`] should handle.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<HandlerEvent>;
}

/// Represents the state of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeState {
    /// Node is pruning the chain.
    Pruning,
    /// Node is syncing the chain.
    Syncing,
    /// Node is actively processing the chain.
    #[default]
    Active,
}

impl NodeState {
    /// Checks if the node is pruning.
    pub const fn is_pruning(&self) -> bool {
        matches!(self, Self::Pruning)
    }

    /// Checks if the node is syncing.
    pub const fn is_syncing(&self) -> bool {
        matches!(self, Self::Syncing)
    }

    /// Checks if the node is active.
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}
