//! Support for handling peer sessions.
use crate::{
    session::handle::{
        ActiveSessionHandle, ActiveSessionMessage, PendingSessionEvent, PendingSessionHandle,
    },
    NodeId,
};
use fnv::FnvHashMap;
use futures::{future::Either, io, FutureExt, StreamExt};
pub use handle::PeerMessageSender;
use reth_ecies::{stream::ECIESStream, ECIESError};
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    UnauthedEthStream,
};
use secp256k1::{SecretKey, SECP256K1};
use std::{
    collections::HashMap,
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
use tracing::{instrument, trace, warn};

mod handle;

/// Internal identifier for active sessions.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq, Hash)]
pub struct SessionId(usize);

/// Manages a set of sessions.
#[must_use = "Session Manager must be polled to process session events."]
pub(crate) struct SessionManager {
    /// Tracks the identifier for the next session.
    next_id: usize,
    /// The secret key used for authenticating sessions.
    secret_key: SecretKey,
    /// The node id of node
    node_id: NodeId,
    /// Size of the command buffer per session.
    session_command_buffer: usize,
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
    active_sessions: HashMap<NodeId, ActiveSessionHandle>,
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
}

// === impl SessionManager ===

impl SessionManager {
    /// Creates a new empty [`SessionManager`].
    pub(crate) fn new(secret_key: SecretKey, config: SessionsConfig) -> Self {
        let (pending_sessions_tx, pending_sessions_rx) = mpsc::channel(config.session_event_buffer);
        let (active_session_tx, active_session_rx) = mpsc::channel(config.session_event_buffer);

        let pk = secret_key.public_key(SECP256K1);
        let node_id = NodeId::from_slice(&pk.serialize_uncompressed()[1..]);

        Self {
            next_id: 0,
            secret_key,
            node_id,
            session_command_buffer: config.session_command_buffer,
            spawned_tasks: Default::default(),
            pending_sessions: Default::default(),
            active_sessions: Default::default(),
            pending_sessions_tx,
            pending_session_rx: ReceiverStream::new(pending_sessions_rx),
            active_session_tx,
            active_session_rx: ReceiverStream::new(active_session_rx),
        }
    }

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
        // TODO(mattsse): enforce limits
        let session_id = self.next_id();
        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let pending_events = self.pending_sessions_tx.clone();
        self.spawn(start_pending_incoming_session(
            disconnect_rx,
            session_id,
            stream,
            pending_events,
            remote_addr,
            self.secret_key,
        ));

        let handle = PendingSessionHandle { disconnect_tx };
        self.pending_sessions.insert(session_id, handle);
        Ok(session_id)
    }

    /// Starts a new pending session from the local node to the given remote node.
    pub(crate) fn dial_outbound(&mut self, remote_addr: SocketAddr, remote_node_id: NodeId) {
        let session_id = self.next_id();
        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let pending_events = self.pending_sessions_tx.clone();
        self.spawn(start_pending_outbound_session(
            disconnect_rx,
            pending_events,
            session_id,
            remote_addr,
            remote_node_id,
            self.secret_key,
        ));

        let handle = PendingSessionHandle { disconnect_tx };
        self.pending_sessions.insert(session_id, handle);
    }

    /// Initiates a shutdown of the channel.
    ///
    /// This will trigger the disconnect on the session task to gracefully terminate. The result
    /// will be picked up by the receiver.
    pub(crate) fn disconnect(&self, node: NodeId) {
        if let Some(session) = self.active_sessions.get(&node) {
            session.disconnect();
        }
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
            Poll::Ready(Some(event)) => {
                return match event {
                    ActiveSessionMessage::Closed { node_id, remote_addr } => {
                        trace!(?node_id, target = "net::session", "closed active session.");
                        let _ = self.active_sessions.remove(&node_id);
                        Poll::Ready(SessionEvent::Disconnected { node_id, remote_addr })
                    }
                    ActiveSessionMessage::ValidMessage { node_id, message } => {
                        // TODO: since all messages are known they should be decoded in the session
                        Poll::Ready(SessionEvent::ValidMessage { node_id, message })
                    }
                    ActiveSessionMessage::InvalidMessage { node_id, capabilities, message } => {
                        Poll::Ready(SessionEvent::InvalidMessage { node_id, message, capabilities })
                    }
                }
            }
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
                PendingSessionEvent::Hello {
                    session_id,
                    node_id: _,
                    capabilities: _,
                    stream: _,
                } => {
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
                PendingSessionEvent::Disconnected { remote_addr, session_id, direction, error } => {
                    trace!(
                        ?session_id,
                        ?remote_addr,
                        target = "net::session",
                        "disconnected pending session"
                    );
                    let _ = self.pending_sessions.remove(&session_id);
                    return match direction {
                        Direction::Incoming => {
                            Poll::Ready(SessionEvent::IncomingPendingSessionClosed {
                                remote_addr,
                                error,
                            })
                        }
                        Direction::Outgoing(node_id) => {
                            Poll::Ready(SessionEvent::OutgoingPendingSessionClosed {
                                remote_addr,
                                node_id,
                                error,
                            })
                        }
                    }
                }
                PendingSessionEvent::OutgoingConnectionError {
                    remote_addr,
                    session_id,
                    node_id,
                    error,
                } => {
                    trace!(
                        ?error,
                        ?session_id,
                        ?remote_addr,
                        ?node_id,
                        target = "net::session",
                        "connection refused"
                    );
                    let _ = self.pending_sessions.remove(&session_id);
                    return Poll::Ready(SessionEvent::IncomingPendingSessionClosed {
                        remote_addr,
                        error: None,
                    })
                }
                PendingSessionEvent::EciesAuthError { remote_addr, session_id, error } => {
                    let _ = self.pending_sessions.remove(&session_id);
                    warn!(
                        ?error,
                        ?session_id,
                        ?remote_addr,
                        target = "net::session",
                        "ecies auth failed"
                    );
                    let _ = self.pending_sessions.remove(&session_id);
                    return Poll::Ready(SessionEvent::IncomingPendingSessionClosed {
                        remote_addr,
                        error: None,
                    })
                }
            }
        }

        Poll::Pending
    }
}

/// Configuration options when creating a [`SessionsManager`].
pub struct SessionsConfig {
    /// Size of the session command buffer (per session task).
    pub session_command_buffer: usize,
    /// Size of the session event channel buffer.
    pub session_event_buffer: usize,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        SessionsConfig {
            // This should be sufficient to slots for handling commands sent to the session task,
            // since the manager is the sender.
            session_command_buffer: 10,
            // This should be greater since the manager is the receiver. The total size will be
            // `buffer + num sessions`. Each session can therefor fit at least 1 message in the
            // channel. The buffer size is additional capacity. The channel is always drained on
            // `poll`.
            session_event_buffer: 64,
        }
    }
}

impl SessionsConfig {
    /// Sets the buffer size for the bounded communication channel between the manager and its
    /// sessions for events emitted by the sessions.
    ///
    /// It is expected, that the background session task will stall if they outpace the manager. The
    /// buffer size provides backpressure on the network I/O.
    pub fn with_session_event_buffer(mut self, n: usize) -> Self {
        self.session_event_buffer = n;
        self
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
    /// Closed an incoming pending session during authentication.
    IncomingPendingSessionClosed { remote_addr: SocketAddr, error: Option<ECIESError> },
    /// Closed an outgoing pending session during authentication.
    OutgoingPendingSessionClosed {
        remote_addr: SocketAddr,
        node_id: NodeId,
        error: Option<ECIESError>,
    },
    /// Failed to establish a tcp stream
    OutgoingConnectionError { remote_addr: SocketAddr, node_id: NodeId, error: io::Error },
    /// Active session was disconnected.
    Disconnected { node_id: NodeId, remote_addr: SocketAddr },
}

/// The error thrown when the max configured limit has been reached and no more connections are
/// accepted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Session limit reached {0}")]
pub struct ExceedsSessionLimit(usize);

/// Starts the authentication process for a connection initiated by a remote peer.
///
/// This will wait for the _incoming_ handshake request and answer it.
async fn start_pending_incoming_session(
    disconnect_rx: oneshot::Receiver<()>,
    session_id: SessionId,
    stream: TcpStream,
    events: mpsc::Sender<PendingSessionEvent>,
    remote_addr: SocketAddr,
    secret_key: SecretKey,
) {
    authenticate(
        disconnect_rx,
        events,
        stream,
        session_id,
        remote_addr,
        secret_key,
        Direction::Incoming,
    )
    .await
}

/// Starts the authentication process for a connection initiated by a remote peer.
#[instrument(skip_all, fields(%remote_addr, node_id), target = "net")]
async fn start_pending_outbound_session(
    disconnect_rx: oneshot::Receiver<()>,
    events: mpsc::Sender<PendingSessionEvent>,
    session_id: SessionId,
    remote_addr: SocketAddr,
    remote_node_id: NodeId,
    secret_key: SecretKey,
) {
    let stream = match TcpStream::connect(remote_addr).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = events
                .send(PendingSessionEvent::OutgoingConnectionError {
                    remote_addr,
                    session_id,
                    node_id: remote_node_id,
                    error,
                })
                .await;
            return
        }
    };
    authenticate(
        disconnect_rx,
        events,
        stream,
        session_id,
        remote_addr,
        secret_key,
        Direction::Outgoing(remote_node_id),
    )
    .await
}

/// The direction of the connection.
#[derive(Debug, Copy, Clone)]
pub(crate) enum Direction {
    /// Incoming connection.
    Incoming,
    /// Outgoing connection to a specific node.
    Outgoing(NodeId),
}

async fn authenticate(
    disconnect_rx: oneshot::Receiver<()>,
    events: mpsc::Sender<PendingSessionEvent>,
    stream: TcpStream,
    session_id: SessionId,
    remote_addr: SocketAddr,
    secret_key: SecretKey,
    direction: Direction,
) {
    let stream = match direction {
        Direction::Incoming => match ECIESStream::incoming(stream, secret_key).await {
            Ok(stream) => stream,
            Err(error) => {
                let _ = events
                    .send(PendingSessionEvent::EciesAuthError { remote_addr, session_id, error })
                    .await;
                return
            }
        },
        Direction::Outgoing(remote_node_id) => {
            match ECIESStream::connect(stream, secret_key, remote_node_id).await {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = events
                        .send(PendingSessionEvent::EciesAuthError {
                            remote_addr,
                            session_id,
                            error,
                        })
                        .await;
                    return
                }
            }
        }
    };

    let unauthed = UnauthedEthStream::new(stream);
    let auth = authenticate_stream(unauthed, session_id, remote_addr, direction).boxed();

    match futures::future::select(disconnect_rx, auth).await {
        Either::Left((_, _)) => {
            let _ = events
                .send(PendingSessionEvent::Disconnected {
                    remote_addr,
                    session_id,
                    direction,
                    error: None,
                })
                .await;
        }
        Either::Right((res, _)) => {
            let _ = events.send(res).await;
        }
    }
}

/// Authenticate the stream via handshake
///
/// On Success return the authenticated stream as [`PendingSessionEvent`]
async fn authenticate_stream(
    _stream: UnauthedEthStream<ECIESStream<TcpStream>>,
    _session_id: SessionId,
    _remote_addr: SocketAddr,
    _direction: Direction,
) -> PendingSessionEvent {
    todo!()
}
