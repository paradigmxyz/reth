//! Support for handling peer sessions.
use crate::{
    capability::{Capabilities, CapabilityMessage},
    session::handle::{
        ActiveSessionHandle, ActiveSessionMessage, PendingSessionEvent, PendingSessionHandle,
    },
    NodeId,
};
use fnv::FnvHashMap;
use futures::StreamExt;
pub use handle::PeerMessageSender;
use std::{
    future::Future,
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_stream::wrappers::ReceiverStream;
use tracing::trace;

mod handle;

/// Internal identifier for active sessions.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq, Hash)]
pub struct SessionId(usize);

/// Manages a set of sessions.
#[must_use = "Session Manager must be polled to process session events."]
pub(crate) struct SessionManager {
    /// Tracks the identifier for the next session.
    next_id: usize,
    /// All spawned session tasks.
    ///
    /// Note: If dropped, the session tasks are aborted.
    spawned_tasks: JoinSet<()>,
    /// All pending session that are currently handshaking, exchanging `Hello`s.
    ///
    /// Events produced during the authentication phase are reported to this manager. Once the
    /// session is authenticated, it can be moved to the `active_session` set.
    pending_sessions: FnvHashMap<SessionId, PendingSessionHandle>,
    /// All active sessions that are ready to exchange messages.
    active_sessions: FnvHashMap<NodeId, ActiveSessionHandle>,
    /// The original Sender half of the [`PendingSessionEvent`] channel.
    ///
    /// When a new (pending) session is created, the corresponding [`PendingSessionHandle`] will
    /// get a clone of this sender half.
    pending_sessions_tx: mpsc::Sender<PendingSessionEvent>,
    /// Receiver half that listens for [`PendingSessionEvent`] produced by pending sessions.
    pending_session_rx: ReceiverStream<PendingSessionEvent>,
    /// The original Sender half of the [`ActiveSessionEvent`] channel.
    ///
    /// When active session state is reached, the corresponding [`ActiveSessionHandle`] will get a
    /// clone of this sender half.
    active_session_tx: mpsc::Sender<ActiveSessionMessage>,
    /// Receiver half that listens for [`ActiveSessionEvent`] produced by pending sessions.
    active_session_rx: ReceiverStream<ActiveSessionMessage>,
    // TODO(mattsse): since new blocks need to be relayed to all nodes, this could either happen
    // through a separate `tokio::sync::broadcast::Sender` or part of the mpsc channels
}

// === impl SessionManager ===

impl SessionManager {
    /// Returns the next unique [`SessionId`].
    fn next_id(&mut self) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;
        SessionId(id)
    }

    /// Spawns the given future onto a new task that is tracked in the `spawned_tasks` [`JoinSet`].
    fn spawn<F>(&mut self, f: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawned_tasks.spawn(async move { f.await });
    }

    /// A incoming TCP connection was received. This starts the authentication process to turn this
    /// stream into an active peer session.
    ///
    /// Returns an error if the configured limit has been reached.
    pub(crate) fn on_incoming(
        &mut self,
        stream: TcpStream,
        remote_addr: SocketAddr,
    ) -> Result<SessionId, ExceedsSessionLimit> {
        // TODO check limits
        let session_id = self.next_id();
        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let pending_events = self.pending_sessions_tx.clone();
        self.spawn(start_pending_incoming_session(
            session_id,
            stream,
            disconnect_rx,
            pending_events,
            remote_addr,
        ));

        let handle = PendingSessionHandle { disconnect_tx };
        self.pending_sessions.insert(session_id, handle);
        Ok(session_id)
    }

    /// Starts a new pending session from the local node to the given remote node.
    pub(crate) fn start_outbound(&mut self, remote_addr: SocketAddr, node_id: NodeId) {
        let session_id = self.next_id();
        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let pending_events = self.pending_sessions_tx.clone();
        self.spawn(start_pending_outbound_session(
            session_id,
            disconnect_rx,
            pending_events,
            remote_addr,
            node_id,
        ));

        let handle = PendingSessionHandle { disconnect_tx };
        self.pending_sessions.insert(session_id, handle);
    }

    /// This polls all the session handles and returns [`SessionEvent`].
    ///
    /// Active sessions are prioritized.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<SessionEvent> {
        // Poll events from active sessions
        match self.active_session_rx.poll_next_unpin(cx) {
            Poll::Pending => {}
            Poll::Ready(None) => {
                unreachable!("Manager holds both channel halves.")
            }
            Poll::Ready(Some(event)) => match event {
                ActiveSessionMessage::Closed { node_id, remote_addr } => {
                    trace!(?node_id, target = "net::session", "closed active session.");
                    let _ = self.active_sessions.remove(&node_id);
                    return Poll::Ready(SessionEvent::Disconnected { node_id, remote_addr })
                }
                ActiveSessionMessage::ValidMessage { node_id, message } => {
                    // TODO: since all messages are known they should be decoded in the session
                    return Poll::Ready(SessionEvent::ValidMessage { node_id, message })
                }
                ActiveSessionMessage::InvalidMessage { node_id, capabilities, message } => {
                    return Poll::Ready(SessionEvent::InvalidMessage {
                        node_id,
                        message,
                        capabilities,
                    })
                }
            },
        }

        // Poll the pending session event stream
        loop {
            let event = match self.pending_session_rx.poll_next_unpin(cx) {
                Poll::Pending => break,
                Poll::Ready(None) => unreachable!("Manager holds both channel halves."),
                Poll::Ready(Some(event)) => event,
            };
            match event {
                PendingSessionEvent::SuccessfulHandshake { remote_addr, session_id } => {
                    trace!(
                        ?session_id,
                        ?remote_addr,
                        target = "net::session",
                        "successful handshake"
                    );
                }
                PendingSessionEvent::Hello { session_id, node_id: _, capabilities: _, stream: _ } => {
                    // move from pending to established.
                    let _ = self.pending_sessions.remove(&session_id);

                    // TODO spawn the authenticated session
                    // let session = ActiveSessionHandle {
                    //     session_id,
                    //     remote_id: node_id,
                    //     established: Instant::now(),
                    //     capabilities,
                    //     commands
                    // };
                    // self.active_sessions.insert(node_id, session);
                    // return Poll::Ready(SessionEvent::SessionAuthenticated {
                    //     node_id,
                    //     capabilities,
                    //     messages: ()
                    // })
                }
                PendingSessionEvent::Disconnected { remote_addr, session_id } => {
                    trace!(
                        ?session_id,
                        ?remote_addr,
                        target = "net::session",
                        "disconnected pending session"
                    );
                    let _ = self.pending_sessions.remove(&session_id);
                    return Poll::Ready(SessionEvent::DisconnectedPending { remote_addr })
                }
            }
        }

        Poll::Pending
    }
}

/// Events produced by the [`SessionManager`]
pub(crate) enum SessionEvent {
    /// A new session was successfully authenticated.
    ///
    /// This session is now able to exchange data.
    SessionAuthenticated {
        node_id: NodeId,
        remote_addr: SocketAddr,
        capabilities: Arc<Capabilities>,
        messages: PeerMessageSender,
    },
    /// A session received a valid message via RLPx.
    ValidMessage {
        node_id: NodeId,
        /// Message received from the peer.
        message: CapabilityMessage,
    },
    /// Received a message that does not match the announced capabilities of the peer.
    InvalidMessage {
        node_id: NodeId,
        /// Announced capabilities of the remote peer.
        capabilities: Arc<Capabilities>,
        /// Message received from the peer.
        message: CapabilityMessage,
    },
    /// Disconnected during pending state.
    DisconnectedPending { remote_addr: SocketAddr },
    /// Active session was disconnected.
    Disconnected { node_id: NodeId, remote_addr: SocketAddr },
}

/// The error thrown when the max configured limit has been reached and no more connections are
/// accepted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Session limit reached {0}")]
pub struct ExceedsSessionLimit(usize);

/// Starts the authentication process for a connection initiated by a remote peer.
async fn start_pending_incoming_session(
    _session_id: SessionId,
    _stream: TcpStream,
    _disconnect_rx: oneshot::Receiver<()>,
    _events: mpsc::Sender<PendingSessionEvent>,
    _remote_addr: SocketAddr,
) {
    // Authenticates the stream, sends `PendingSessionEvent` updates back, sends Disconnect if
    // disconnect trigger was fired.
    todo!()
}

/// Starts the authentication process for a connection initiated by a remote peer.
async fn start_pending_outbound_session(
    _session_id: SessionId,
    _disconnect_rx: oneshot::Receiver<()>,
    _events: mpsc::Sender<PendingSessionEvent>,
    _remote_addr: SocketAddr,
    _node_id: NodeId,
) {
}
