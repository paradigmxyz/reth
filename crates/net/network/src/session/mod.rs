//! Support for handling peer sessions.

use crate::{message::PeerMessage, metrics::SessionManagerMetrics, session::active::ActiveSession};
use counter::SessionCounter;
use futures::{future::Either, io, FutureExt, StreamExt};
use reth_ecies::{stream::ECIESStream, ECIESError};
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    errors::EthStreamError,
    DisconnectReason, EthVersion, HelloMessageWithProtocols, Status, UnauthedEthStream,
    UnauthedP2PStream,
};
use reth_metrics::common::mpsc::MeteredPollSender;
use reth_network_peers::PeerId;
use reth_network_types::SessionsConfig;
use reth_primitives::{ForkFilter, ForkId, ForkTransition, Head};
use reth_tasks::TaskSpawner;
use rustc_hash::FxHashMap;
use secp256k1::SecretKey;
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    sync::{atomic::AtomicU64, Arc},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::PollSender;
use tracing::{debug, instrument, trace};

mod active;
mod conn;
mod counter;
mod handle;
pub use crate::message::PeerRequestSender;
use crate::protocol::{IntoRlpxSubProtocol, RlpxSubProtocolHandlers, RlpxSubProtocols};
pub use conn::EthRlpxConnection;
pub use handle::{
    ActiveSessionHandle, ActiveSessionMessage, PendingSessionEvent, PendingSessionHandle,
    SessionCommand,
};
use reth_eth_wire::multiplex::RlpxProtocolMultiplexer;
pub use reth_network_api::{Direction, PeerInfo};
/// Internal identifier for active sessions.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq, Hash)]
pub struct SessionId(usize);

/// Manages a set of sessions.
#[must_use = "Session Manager must be polled to process session events."]
#[derive(Debug)]
pub struct SessionManager {
    /// Tracks the identifier for the next session.
    next_id: usize,
    /// Keeps track of all sessions
    counter: SessionCounter,
    ///  The maximum initial time an [`ActiveSession`] waits for a response from the peer before it
    /// responds to an _internal_ request with a `TimeoutError`
    initial_internal_request_timeout: Duration,
    /// If an [`ActiveSession`] does not receive a response at all within this duration then it is
    /// considered a protocol violation and the session will initiate a drop.
    protocol_breach_request_timeout: Duration,
    /// The timeout after which a pending session attempt is considered failed.
    pending_session_timeout: Duration,
    /// The secret key used for authenticating sessions.
    secret_key: SecretKey,
    /// The `Status` message to send to peers.
    status: Status,
    /// The `HelloMessage` message to send to peers.
    hello_message: HelloMessageWithProtocols,
    /// The [`ForkFilter`] used to validate the peer's `Status` message.
    fork_filter: ForkFilter,
    /// Size of the command buffer per session.
    session_command_buffer: usize,
    /// The executor for spawned tasks.
    executor: Box<dyn TaskSpawner>,
    /// All pending session that are currently handshaking, exchanging `Hello`s.
    ///
    /// Events produced during the authentication phase are reported to this manager. Once the
    /// session is authenticated, it can be moved to the `active_session` set.
    pending_sessions: FxHashMap<SessionId, PendingSessionHandle>,
    /// All active sessions that are ready to exchange messages.
    active_sessions: HashMap<PeerId, ActiveSessionHandle>,
    /// The original Sender half of the [`PendingSessionEvent`] channel.
    ///
    /// When a new (pending) session is created, the corresponding [`PendingSessionHandle`] will
    /// get a clone of this sender half.
    pending_sessions_tx: mpsc::Sender<PendingSessionEvent>,
    /// Receiver half that listens for [`PendingSessionEvent`] produced by pending sessions.
    pending_session_rx: ReceiverStream<PendingSessionEvent>,
    /// The original Sender half of the [`ActiveSessionMessage`] channel.
    ///
    /// When active session state is reached, the corresponding [`ActiveSessionHandle`] will get a
    /// clone of this sender half.
    active_session_tx: MeteredPollSender<ActiveSessionMessage>,
    /// Receiver half that listens for [`ActiveSessionMessage`] produced by pending sessions.
    active_session_rx: ReceiverStream<ActiveSessionMessage>,
    /// Additional `RLPx` sub-protocols to be used by the session manager.
    extra_protocols: RlpxSubProtocols,
    /// Metrics for the session manager.
    metrics: SessionManagerMetrics,
}

// === impl SessionManager ===

impl SessionManager {
    /// Creates a new empty [`SessionManager`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        secret_key: SecretKey,
        config: SessionsConfig,
        executor: Box<dyn TaskSpawner>,
        status: Status,
        hello_message: HelloMessageWithProtocols,
        fork_filter: ForkFilter,
        extra_protocols: RlpxSubProtocols,
    ) -> Self {
        let (pending_sessions_tx, pending_sessions_rx) = mpsc::channel(config.session_event_buffer);
        let (active_session_tx, active_session_rx) = mpsc::channel(config.session_event_buffer);
        let active_session_tx = PollSender::new(active_session_tx);

        Self {
            next_id: 0,
            counter: SessionCounter::new(config.limits),
            initial_internal_request_timeout: config.initial_internal_request_timeout,
            protocol_breach_request_timeout: config.protocol_breach_request_timeout,
            pending_session_timeout: config.pending_session_timeout,
            secret_key,
            status,
            hello_message,
            fork_filter,
            session_command_buffer: config.session_command_buffer,
            executor,
            pending_sessions: Default::default(),
            active_sessions: Default::default(),
            pending_sessions_tx,
            pending_session_rx: ReceiverStream::new(pending_sessions_rx),
            active_session_tx: MeteredPollSender::new(active_session_tx, "network_active_session"),
            active_session_rx: ReceiverStream::new(active_session_rx),
            extra_protocols,
            metrics: Default::default(),
        }
    }

    /// Check whether the provided [`ForkId`] is compatible based on the validation rules in
    /// `EIP-2124`.
    pub fn is_valid_fork_id(&self, fork_id: ForkId) -> bool {
        self.fork_filter.validate(fork_id).is_ok()
    }

    /// Returns the next unique [`SessionId`].
    fn next_id(&mut self) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;
        SessionId(id)
    }

    /// Returns the current status of the session.
    pub const fn status(&self) -> Status {
        self.status
    }

    /// Returns the secret key used for authenticating sessions.
    pub const fn secret_key(&self) -> SecretKey {
        self.secret_key
    }

    /// Returns a borrowed reference to the active sessions.
    pub const fn active_sessions(&self) -> &HashMap<PeerId, ActiveSessionHandle> {
        &self.active_sessions
    }

    /// Returns the session hello message.
    pub fn hello_message(&self) -> HelloMessageWithProtocols {
        self.hello_message.clone()
    }

    /// Adds an additional protocol handler to the `RLPx` sub-protocol list.
    pub(crate) fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol) {
        self.extra_protocols.push(protocol)
    }

    /// Returns the number of currently pending connections.
    #[inline]
    pub(crate) fn num_pending_connections(&self) -> usize {
        self.pending_sessions.len()
    }

    /// Spawns the given future onto a new task that is tracked in the `spawned_tasks`
    /// [`JoinSet`](tokio::task::JoinSet).
    fn spawn<F>(&self, f: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.executor.spawn(f.boxed());
    }

    /// Invoked on a received status update.
    ///
    /// If the updated activated another fork, this will return a [`ForkTransition`] and updates the
    /// active [`ForkId`]. See also [`ForkFilter::set_head`].
    pub(crate) fn on_status_update(&mut self, head: Head) -> Option<ForkTransition> {
        self.status.blockhash = head.hash;
        self.status.total_difficulty = head.total_difficulty;
        let transition = self.fork_filter.set_head(head);
        self.status.forkid = self.fork_filter.current();
        transition
    }

    /// An incoming TCP connection was received. This starts the authentication process to turn this
    /// stream into an active peer session.
    ///
    /// Returns an error if the configured limit has been reached.
    pub(crate) fn on_incoming(
        &mut self,
        stream: TcpStream,
        remote_addr: SocketAddr,
    ) -> Result<SessionId, ExceedsSessionLimit> {
        self.counter.ensure_pending_inbound()?;

        let session_id = self.next_id();

        trace!(
            target: "net::session",
            ?remote_addr,
            ?session_id,
            "new pending incoming session"
        );

        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let pending_events = self.pending_sessions_tx.clone();
        let secret_key = self.secret_key;
        let hello_message = self.hello_message.clone();
        let status = self.status;
        let fork_filter = self.fork_filter.clone();
        let extra_handlers = self.extra_protocols.on_incoming(remote_addr);
        self.spawn(pending_session_with_timeout(
            self.pending_session_timeout,
            session_id,
            remote_addr,
            Direction::Incoming,
            pending_events.clone(),
            start_pending_incoming_session(
                disconnect_rx,
                session_id,
                stream,
                pending_events,
                remote_addr,
                secret_key,
                hello_message,
                status,
                fork_filter,
                extra_handlers,
            ),
        ));

        let handle = PendingSessionHandle {
            disconnect_tx: Some(disconnect_tx),
            direction: Direction::Incoming,
        };
        self.pending_sessions.insert(session_id, handle);
        self.counter.inc_pending_inbound();
        Ok(session_id)
    }

    /// Starts a new pending session from the local node to the given remote node.
    pub fn dial_outbound(&mut self, remote_addr: SocketAddr, remote_peer_id: PeerId) {
        // The error can be dropped because no dial will be made if it would exceed the limit
        if self.counter.ensure_pending_outbound().is_ok() {
            let session_id = self.next_id();
            let (disconnect_tx, disconnect_rx) = oneshot::channel();
            let pending_events = self.pending_sessions_tx.clone();
            let secret_key = self.secret_key;
            let hello_message = self.hello_message.clone();
            let fork_filter = self.fork_filter.clone();
            let status = self.status;
            let extra_handlers = self.extra_protocols.on_outgoing(remote_addr, remote_peer_id);
            self.spawn(pending_session_with_timeout(
                self.pending_session_timeout,
                session_id,
                remote_addr,
                Direction::Outgoing(remote_peer_id),
                pending_events.clone(),
                start_pending_outbound_session(
                    disconnect_rx,
                    pending_events,
                    session_id,
                    remote_addr,
                    remote_peer_id,
                    secret_key,
                    hello_message,
                    status,
                    fork_filter,
                    extra_handlers,
                ),
            ));

            let handle = PendingSessionHandle {
                disconnect_tx: Some(disconnect_tx),
                direction: Direction::Outgoing(remote_peer_id),
            };
            self.pending_sessions.insert(session_id, handle);
            self.counter.inc_pending_outbound();
        }
    }

    /// Initiates a shutdown of the channel.
    ///
    /// This will trigger the disconnect on the session task to gracefully terminate. The result
    /// will be picked up by the receiver.
    pub fn disconnect(&self, node: PeerId, reason: Option<DisconnectReason>) {
        if let Some(session) = self.active_sessions.get(&node) {
            session.disconnect(reason);
        }
    }

    /// Initiates a shutdown of all sessions.
    ///
    /// It will trigger the disconnect on all the session tasks to gracefully terminate. The result
    /// will be picked by the receiver.
    pub fn disconnect_all(&self, reason: Option<DisconnectReason>) {
        for session in self.active_sessions.values() {
            session.disconnect(reason);
        }
    }

    /// Disconnects all pending sessions.
    pub fn disconnect_all_pending(&mut self) {
        for session in self.pending_sessions.values_mut() {
            session.disconnect();
        }
    }

    /// Sends a message to the peer's session
    pub fn send_message(&mut self, peer_id: &PeerId, msg: PeerMessage) {
        if let Some(session) = self.active_sessions.get_mut(peer_id) {
            let _ = session.commands_to_session.try_send(SessionCommand::Message(msg));
        }
    }

    /// Removes the [`PendingSessionHandle`] if it exists.
    fn remove_pending_session(&mut self, id: &SessionId) -> Option<PendingSessionHandle> {
        let session = self.pending_sessions.remove(id)?;
        self.counter.dec_pending(&session.direction);
        Some(session)
    }

    /// Removes the [`PendingSessionHandle`] if it exists.
    fn remove_active_session(&mut self, id: &PeerId) -> Option<ActiveSessionHandle> {
        let session = self.active_sessions.remove(id)?;
        self.counter.dec_active(&session.direction);
        Some(session)
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
                    ActiveSessionMessage::Disconnected { peer_id, remote_addr } => {
                        trace!(
                            target: "net::session",
                            ?peer_id,
                            "gracefully disconnected active session."
                        );
                        self.remove_active_session(&peer_id);
                        Poll::Ready(SessionEvent::Disconnected { peer_id, remote_addr })
                    }
                    ActiveSessionMessage::ClosedOnConnectionError {
                        peer_id,
                        remote_addr,
                        error,
                    } => {
                        trace!(target: "net::session", ?peer_id, %error,"closed session.");
                        self.remove_active_session(&peer_id);
                        Poll::Ready(SessionEvent::SessionClosedOnConnectionError {
                            remote_addr,
                            peer_id,
                            error,
                        })
                    }
                    ActiveSessionMessage::ValidMessage { peer_id, message } => {
                        Poll::Ready(SessionEvent::ValidMessage { peer_id, message })
                    }
                    ActiveSessionMessage::InvalidMessage { peer_id, capabilities, message } => {
                        Poll::Ready(SessionEvent::InvalidMessage { peer_id, message, capabilities })
                    }
                    ActiveSessionMessage::BadMessage { peer_id } => {
                        Poll::Ready(SessionEvent::BadMessage { peer_id })
                    }
                    ActiveSessionMessage::ProtocolBreach { peer_id } => {
                        Poll::Ready(SessionEvent::ProtocolBreach { peer_id })
                    }
                }
            }
        }

        // Poll the pending session event stream
        let event = match self.pending_session_rx.poll_next_unpin(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(None) => unreachable!("Manager holds both channel halves."),
            Poll::Ready(Some(event)) => event,
        };
        match event {
            PendingSessionEvent::Established {
                session_id,
                remote_addr,
                local_addr,
                peer_id,
                capabilities,
                conn,
                status,
                direction,
                client_id,
            } => {
                // move from pending to established.
                self.remove_pending_session(&session_id);

                // If there's already a session to the peer then we disconnect right away
                if self.active_sessions.contains_key(&peer_id) {
                    trace!(
                        target: "net::session",
                        ?session_id,
                        ?remote_addr,
                        ?peer_id,
                        ?direction,
                        "already connected"
                    );

                    self.spawn(async move {
                        // send a disconnect message
                        let _ =
                            conn.into_inner().disconnect(DisconnectReason::AlreadyConnected).await;
                    });

                    return Poll::Ready(SessionEvent::AlreadyConnected {
                        peer_id,
                        remote_addr,
                        direction,
                    })
                }

                let (commands_to_session, commands_rx) = mpsc::channel(self.session_command_buffer);

                let (to_session_tx, messages_rx) = mpsc::channel(self.session_command_buffer);

                let messages = PeerRequestSender::new(peer_id, to_session_tx);

                let timeout = Arc::new(AtomicU64::new(
                    self.initial_internal_request_timeout.as_millis() as u64,
                ));

                // negotiated version
                let version = conn.version();

                let session = ActiveSession {
                    next_id: 0,
                    remote_peer_id: peer_id,
                    remote_addr,
                    remote_capabilities: Arc::clone(&capabilities),
                    session_id,
                    commands_rx: ReceiverStream::new(commands_rx),
                    to_session_manager: self.active_session_tx.clone(),
                    pending_message_to_session: None,
                    internal_request_tx: ReceiverStream::new(messages_rx).fuse(),
                    inflight_requests: Default::default(),
                    conn,
                    queued_outgoing: Default::default(),
                    received_requests_from_remote: Default::default(),
                    internal_request_timeout_interval: tokio::time::interval(
                        self.initial_internal_request_timeout,
                    ),
                    internal_request_timeout: Arc::clone(&timeout),
                    protocol_breach_request_timeout: self.protocol_breach_request_timeout,
                    terminate_message: None,
                };

                self.spawn(session);

                let client_version = client_id.into();
                let handle = ActiveSessionHandle {
                    status: status.clone(),
                    direction,
                    session_id,
                    remote_id: peer_id,
                    version,
                    established: Instant::now(),
                    capabilities: Arc::clone(&capabilities),
                    commands_to_session,
                    client_version: Arc::clone(&client_version),
                    remote_addr,
                    local_addr,
                };

                self.active_sessions.insert(peer_id, handle);
                self.counter.inc_active(&direction);

                if direction.is_outgoing() {
                    self.metrics.total_dial_successes.increment(1);
                }

                Poll::Ready(SessionEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    version,
                    capabilities,
                    status,
                    messages,
                    direction,
                    timeout,
                })
            }
            PendingSessionEvent::Disconnected { remote_addr, session_id, direction, error } => {
                trace!(
                    target: "net::session",
                    ?session_id,
                    ?remote_addr,
                    ?error,
                    "disconnected pending session"
                );
                self.remove_pending_session(&session_id);
                match direction {
                    Direction::Incoming => {
                        Poll::Ready(SessionEvent::IncomingPendingSessionClosed {
                            remote_addr,
                            error,
                        })
                    }
                    Direction::Outgoing(peer_id) => {
                        Poll::Ready(SessionEvent::OutgoingPendingSessionClosed {
                            remote_addr,
                            peer_id,
                            error,
                        })
                    }
                }
            }
            PendingSessionEvent::OutgoingConnectionError {
                remote_addr,
                session_id,
                peer_id,
                error,
            } => {
                trace!(
                    target: "net::session",
                    %error,
                    ?session_id,
                    ?remote_addr,
                    ?peer_id,
                    "connection refused"
                );
                self.remove_pending_session(&session_id);
                Poll::Ready(SessionEvent::OutgoingConnectionError { remote_addr, peer_id, error })
            }
            PendingSessionEvent::EciesAuthError { remote_addr, session_id, error, direction } => {
                trace!(
                    target: "net::session",
                    %error,
                    ?session_id,
                    ?remote_addr,
                    "ecies auth failed"
                );
                self.remove_pending_session(&session_id);
                match direction {
                    Direction::Incoming => {
                        Poll::Ready(SessionEvent::IncomingPendingSessionClosed {
                            remote_addr,
                            error: Some(PendingSessionHandshakeError::Ecies(error)),
                        })
                    }
                    Direction::Outgoing(peer_id) => {
                        Poll::Ready(SessionEvent::OutgoingPendingSessionClosed {
                            remote_addr,
                            peer_id,
                            error: Some(PendingSessionHandshakeError::Ecies(error)),
                        })
                    }
                }
            }
        }
    }
}

/// Events produced by the [`SessionManager`]
#[derive(Debug)]
pub enum SessionEvent {
    /// A new session was successfully authenticated.
    ///
    /// This session is now able to exchange data.
    SessionEstablished {
        /// The remote node's public key
        peer_id: PeerId,
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The user agent of the remote node, usually containing the client name and version
        client_version: Arc<str>,
        /// The capabilities the remote node has announced
        capabilities: Arc<Capabilities>,
        /// negotiated eth version
        version: EthVersion,
        /// The Status message the peer sent during the `eth` handshake
        status: Arc<Status>,
        /// The channel for sending messages to the peer with the session
        messages: PeerRequestSender,
        /// The direction of the session, either `Inbound` or `Outgoing`
        direction: Direction,
        /// The maximum time that the session waits for a response from the peer before timing out
        /// the connection
        timeout: Arc<AtomicU64>,
    },
    /// The peer was already connected with another session.
    AlreadyConnected {
        /// The remote node's public key
        peer_id: PeerId,
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The direction of the session, either `Inbound` or `Outgoing`
        direction: Direction,
    },
    /// A session received a valid message via `RLPx`.
    ValidMessage {
        /// The remote node's public key
        peer_id: PeerId,
        /// Message received from the peer.
        message: PeerMessage,
    },
    /// Received a message that does not match the announced capabilities of the peer.
    InvalidMessage {
        /// The remote node's public key
        peer_id: PeerId,
        /// Announced capabilities of the remote peer.
        capabilities: Arc<Capabilities>,
        /// Message received from the peer.
        message: CapabilityMessage,
    },
    /// Received a bad message from the peer.
    BadMessage {
        /// Identifier of the remote peer.
        peer_id: PeerId,
    },
    /// Remote peer is considered in protocol violation
    ProtocolBreach {
        /// Identifier of the remote peer.
        peer_id: PeerId,
    },
    /// Closed an incoming pending session during handshaking.
    IncomingPendingSessionClosed {
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The pending handshake session error that caused the session to close
        error: Option<PendingSessionHandshakeError>,
    },
    /// Closed an outgoing pending session during handshaking.
    OutgoingPendingSessionClosed {
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The remote node's public key
        peer_id: PeerId,
        /// The pending handshake session error that caused the session to close
        error: Option<PendingSessionHandshakeError>,
    },
    /// Failed to establish a tcp stream
    OutgoingConnectionError {
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The remote node's public key
        peer_id: PeerId,
        /// The error that caused the outgoing connection to fail
        error: io::Error,
    },
    /// Session was closed due to an error
    SessionClosedOnConnectionError {
        /// The id of the remote peer.
        peer_id: PeerId,
        /// The socket we were connected to.
        remote_addr: SocketAddr,
        /// The error that caused the session to close
        error: EthStreamError,
    },
    /// Active session was gracefully disconnected.
    Disconnected {
        /// The remote node's public key
        peer_id: PeerId,
        /// The remote node's socket address that we were connected to
        remote_addr: SocketAddr,
    },
}

/// Errors that can occur during handshaking/authenticating the underlying streams.
#[derive(Debug, thiserror::Error)]
pub enum PendingSessionHandshakeError {
    /// The pending session failed due to an error while establishing the `eth` stream
    #[error(transparent)]
    Eth(EthStreamError),
    /// The pending session failed due to an error while establishing the ECIES stream
    #[error(transparent)]
    Ecies(ECIESError),
    /// Thrown when the authentication timed out
    #[error("authentication timed out")]
    Timeout,
}

impl PendingSessionHandshakeError {
    /// Returns the [`DisconnectReason`] if the error is a disconnect message
    pub const fn as_disconnected(&self) -> Option<DisconnectReason> {
        match self {
            Self::Eth(eth_err) => eth_err.as_disconnected(),
            _ => None,
        }
    }
}

/// The error thrown when the max configured limit has been reached and no more connections are
/// accepted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("session limit reached {0}")]
pub struct ExceedsSessionLimit(pub(crate) u32);

/// Starts a pending session authentication with a timeout.
pub(crate) async fn pending_session_with_timeout<F>(
    timeout: Duration,
    session_id: SessionId,
    remote_addr: SocketAddr,
    direction: Direction,
    events: mpsc::Sender<PendingSessionEvent>,
    f: F,
) where
    F: Future<Output = ()>,
{
    if tokio::time::timeout(timeout, f).await.is_err() {
        debug!(target: "net::session", ?remote_addr, ?direction, "pending session timed out");
        let event = PendingSessionEvent::Disconnected {
            remote_addr,
            session_id,
            direction,
            error: Some(PendingSessionHandshakeError::Timeout),
        };
        let _ = events.send(event).await;
    }
}

/// Starts the authentication process for a connection initiated by a remote peer.
///
/// This will wait for the _incoming_ handshake request and answer it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_pending_incoming_session(
    disconnect_rx: oneshot::Receiver<()>,
    session_id: SessionId,
    stream: TcpStream,
    events: mpsc::Sender<PendingSessionEvent>,
    remote_addr: SocketAddr,
    secret_key: SecretKey,
    hello: HelloMessageWithProtocols,
    status: Status,
    fork_filter: ForkFilter,
    extra_handlers: RlpxSubProtocolHandlers,
) {
    authenticate(
        disconnect_rx,
        events,
        stream,
        session_id,
        remote_addr,
        secret_key,
        Direction::Incoming,
        hello,
        status,
        fork_filter,
        extra_handlers,
    )
    .await
}

/// Starts the authentication process for a connection initiated by a remote peer.
#[instrument(skip_all, fields(%remote_addr, peer_id), target = "net")]
#[allow(clippy::too_many_arguments)]
async fn start_pending_outbound_session(
    disconnect_rx: oneshot::Receiver<()>,
    events: mpsc::Sender<PendingSessionEvent>,
    session_id: SessionId,
    remote_addr: SocketAddr,
    remote_peer_id: PeerId,
    secret_key: SecretKey,
    hello: HelloMessageWithProtocols,
    status: Status,
    fork_filter: ForkFilter,
    extra_handlers: RlpxSubProtocolHandlers,
) {
    let stream = match TcpStream::connect(remote_addr).await {
        Ok(stream) => {
            if let Err(err) = stream.set_nodelay(true) {
                tracing::warn!(target: "net::session", "set nodelay failed: {:?}", err);
            }
            stream
        }
        Err(error) => {
            let _ = events
                .send(PendingSessionEvent::OutgoingConnectionError {
                    remote_addr,
                    session_id,
                    peer_id: remote_peer_id,
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
        Direction::Outgoing(remote_peer_id),
        hello,
        status,
        fork_filter,
        extra_handlers,
    )
    .await
}

/// Authenticates a session
#[allow(clippy::too_many_arguments)]
async fn authenticate(
    disconnect_rx: oneshot::Receiver<()>,
    events: mpsc::Sender<PendingSessionEvent>,
    stream: TcpStream,
    session_id: SessionId,
    remote_addr: SocketAddr,
    secret_key: SecretKey,
    direction: Direction,
    hello: HelloMessageWithProtocols,
    status: Status,
    fork_filter: ForkFilter,
    extra_handlers: RlpxSubProtocolHandlers,
) {
    let local_addr = stream.local_addr().ok();
    let stream = match get_eciess_stream(stream, secret_key, direction).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = events
                .send(PendingSessionEvent::EciesAuthError {
                    remote_addr,
                    session_id,
                    error,
                    direction,
                })
                .await;
            return
        }
    };

    let unauthed = UnauthedP2PStream::new(stream);

    let auth = authenticate_stream(
        unauthed,
        session_id,
        remote_addr,
        local_addr,
        direction,
        hello,
        status,
        fork_filter,
        extra_handlers,
    )
    .boxed();

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

/// Returns an [`ECIESStream`] if it can be built. If not, send a
/// [`PendingSessionEvent::EciesAuthError`] and returns `None`
async fn get_eciess_stream<Io: AsyncRead + AsyncWrite + Unpin>(
    stream: Io,
    secret_key: SecretKey,
    direction: Direction,
) -> Result<ECIESStream<Io>, ECIESError> {
    match direction {
        Direction::Incoming => ECIESStream::incoming(stream, secret_key).await,
        Direction::Outgoing(remote_peer_id) => {
            ECIESStream::connect(stream, secret_key, remote_peer_id).await
        }
    }
}

/// Authenticate the stream via handshake
///
/// On Success return the authenticated stream as [`PendingSessionEvent`].
///
/// If additional [`RlpxSubProtocolHandlers`] are provided, the hello message will be updated to
/// also negotiate the additional protocols.
#[allow(clippy::too_many_arguments)]
async fn authenticate_stream(
    stream: UnauthedP2PStream<ECIESStream<TcpStream>>,
    session_id: SessionId,
    remote_addr: SocketAddr,
    local_addr: Option<SocketAddr>,
    direction: Direction,
    mut hello: HelloMessageWithProtocols,
    mut status: Status,
    fork_filter: ForkFilter,
    mut extra_handlers: RlpxSubProtocolHandlers,
) -> PendingSessionEvent {
    // Add extra protocols to the hello message
    extra_handlers.retain(|handler| hello.try_add_protocol(handler.protocol()).is_ok());

    // conduct the p2p handshake and return the authenticated stream
    let (p2p_stream, their_hello) = match stream.handshake(hello).await {
        Ok(stream_res) => stream_res,
        Err(err) => {
            return PendingSessionEvent::Disconnected {
                remote_addr,
                session_id,
                direction,
                error: Some(PendingSessionHandshakeError::Eth(err.into())),
            }
        }
    };

    // Ensure we negotiated mandatory eth protocol
    let eth_version = match p2p_stream.shared_capabilities().eth_version() {
        Ok(version) => version,
        Err(err) => {
            return PendingSessionEvent::Disconnected {
                remote_addr,
                session_id,
                direction,
                error: Some(PendingSessionHandshakeError::Eth(err.into())),
            }
        }
    };

    let (conn, their_status) = if p2p_stream.shared_capabilities().len() == 1 {
        // if the hello handshake was successful we can try status handshake
        //
        // Before trying status handshake, set up the version to negotiated shared version
        status.set_eth_version(eth_version);
        let eth_unauthed = UnauthedEthStream::new(p2p_stream);
        let (eth_stream, their_status) = match eth_unauthed.handshake(status, fork_filter).await {
            Ok(stream_res) => stream_res,
            Err(err) => {
                return PendingSessionEvent::Disconnected {
                    remote_addr,
                    session_id,
                    direction,
                    error: Some(PendingSessionHandshakeError::Eth(err)),
                }
            }
        };
        (eth_stream.into(), their_status)
    } else {
        // Multiplex the stream with the extra protocols
        let mut multiplex_stream = RlpxProtocolMultiplexer::new(p2p_stream);

        // install additional handlers
        for handler in extra_handlers.into_iter() {
            let cap = handler.protocol().cap;
            let remote_peer_id = their_hello.id;
            multiplex_stream
                .install_protocol(&cap, move |conn| {
                    handler.into_connection(direction, remote_peer_id, conn)
                })
                .ok();
        }

        let (multiplex_stream, their_status) =
            match multiplex_stream.into_eth_satellite_stream(status, fork_filter).await {
                Ok((multiplex_stream, their_status)) => (multiplex_stream, their_status),
                Err(err) => {
                    return PendingSessionEvent::Disconnected {
                        remote_addr,
                        session_id,
                        direction,
                        error: Some(PendingSessionHandshakeError::Eth(err)),
                    }
                }
            };

        (multiplex_stream.into(), their_status)
    };

    PendingSessionEvent::Established {
        session_id,
        remote_addr,
        local_addr,
        peer_id: their_hello.id,
        capabilities: Arc::new(Capabilities::from(their_hello.capabilities)),
        status: Arc::new(their_status),
        conn,
        direction,
        client_id: their_hello.client_version,
    }
}
