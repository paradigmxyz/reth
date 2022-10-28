use crate::{
    listener::{ConnectionListener, ListenerEvent},
    session::{SessionId, SessionManager},
    state::NetworkState,
    NodeId,
};
use futures::Stream;
use reth_interfaces::provider::BlockProvider;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tracing::warn;

/// Contains the connectivity related state of the network.
///
/// A swarm emits [`SwarmEvent`]s when polled.
#[must_use = "Swarm does nothing unless polled"]
pub struct Swarm<C> {
    /// Listens for new incoming connections.
    incoming: ConnectionListener,
    /// All sessions.
    sessions: SessionManager,
    /// Tracks the entire state of the network and handles events received from the sessions.
    state: NetworkState<C>,
}

// === impl Swarm ===

impl<C> Swarm<C>
where
    C: BlockProvider,
{
    /// Mutable access to the state.
    pub(crate) fn state_mut(&mut self) -> &mut NetworkState<C> {
        &mut self.state
    }

    /// Callback for events produced by [`ConnectionListener`].
    ///
    /// Depending on the event, this will produce a new [`SwarmEvent`].
    fn on_connection(&mut self, event: ListenerEvent) -> Option<SwarmEvent> {
        match event {
            ListenerEvent::Error(err) => return Some(SwarmEvent::TcpListenerError(err)),
            ListenerEvent::ListenerClosed { local_address: address } => {
                return Some(SwarmEvent::TcpListenerClosed { address })
            }
            ListenerEvent::Incoming { stream, remote_addr } => {
                match self.sessions.on_incoming(stream, remote_addr) {
                    Ok(session_id) => {
                        return Some(SwarmEvent::IncomingTcpConnection { session_id, remote_addr })
                    }
                    Err(err) => {
                        warn!(?err, "Incoming connection rejected");
                    }
                }
            }
        }
        None
    }
}

impl<C> Stream for Swarm<C>
where
    C: BlockProvider,
{
    type Item = SwarmEvent;

    /// This advances all components.
    ///
    /// Processes, delegates (internal) commands received from the [`NetworkManager`], then polls
    /// the [`SessionManager`] which yields messages produced by individual peer sessions that are
    /// then handled. Least priority are incoming connections that are handled and delegated to
    /// the [`SessionManager`] to turn them into a session.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // poll all sessions
            match this.sessions.poll(cx) {
                Poll::Pending => {}
                Poll::Ready(_event) => {
                    // handle event
                }
            }

            // poll listener for incoming connections
            match Pin::new(&mut this.incoming).poll(cx) {
                Poll::Pending => {}
                Poll::Ready(event) => {
                    if let Some(event) = this.on_connection(event) {
                        return Poll::Ready(Some(event))
                    }
                    continue
                }
            }

            return Poll::Pending
        }
    }
}

/// All events created or delegated by the [`Swarm`] that represents changes to the state of the
/// network.
pub enum SwarmEvent {
    /// Events related to the actual protocol
    ///
    /// TODO this could be requests for eth-wire, or general protocol related info
    ProtocolEvent(ProtocolEvent),
    /// The underlying tcp listener closed.
    TcpListenerClosed {
        /// Address of the closed listener.
        address: SocketAddr,
    },
    /// The underlying tcp listener encountered an error that we bubble up.
    TcpListenerError(io::Error),
    /// Received an incoming tcp connection.
    ///
    /// This represents the first step in the session authentication process. The swarm will
    /// produce subsequent events once the stream has been authenticated, or was rejected.
    IncomingTcpConnection {
        /// The internal session identifier under which this connection is currently tracked.
        session_id: SessionId,
        /// Address of the remote peer.
        remote_addr: SocketAddr,
    },
    // TODO variants for discovered peers so they get bubbled up to the manager
}

/// Various protocol related event types bubbled up from a session that need to be handled by the
/// network.
pub enum ProtocolEvent {
    EthWire(EthWireMessage),
}

pub enum EthWireMessage {
    NewBlock,

    /// Received a `GetBlockHeaders` that needs to be answered
    GetBlockHeadersRequest {
        /// Node that requested the headers
        target: NodeId,
        /// sender half of the channel to send the response back to the session
        response: oneshot::Sender<()>,
    }, // TODO all variants
}
