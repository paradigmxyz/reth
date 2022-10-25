use crate::{connections::Connections, listener::ConnectionListener};
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
    /// Active connections.
    connections: Connections,
    // TODO add discovery update stream
    // TODO this provides info about connected peers
}

impl Stream for Swarm {
    type Item = SwarmEvent;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

/// All events created or delegated by the [`Swarm`] that represents changes to the state of the
/// network.
pub enum SwarmEvent {}
