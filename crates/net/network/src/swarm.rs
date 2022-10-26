use crate::{listener::ConnectionListener, session::SessionManager};
use futures::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Contains the connectivity related state of the network.
///
/// A swarm emits [`SwarmEvent`]s when polled.
#[must_use = "Swarm does nothing unless polled"]
pub struct Swarm {
    /// Listens for new incoming connections.
    incoming: ConnectionListener,
    /// All sessions.
    sessions: SessionManager,
    // TODO add discovery update stream
    // TODO this provides info about connected peers
}

impl Stream for Swarm {
    type Item = SwarmEvent;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // TODO advance all components:
        // 1. incoming commands (from manager), 2. sessions, 3. incoming connection listener

        // This should receive local events, either from the `NetworkManager` or via some channel,
        // then distribute them to the sessions. this can be a response to one specific peer, such
        // as NewPooledTransactions or a broadcast for all sessions like NewBlock

        todo!()
    }
}

/// All events created or delegated by the [`Swarm`] that represents changes to the state of the
/// network.
pub enum SwarmEvent {}
