//! Support for handling peer sessions.
use crate::{
    message::PeerMessage,
    session::{
        active::ActiveSession,
        config::SessionCounter,
        handle::{
            ActiveSessionHandle, ActiveSessionMessage, PendingSessionEvent, PendingSessionHandle,
            SessionCommand,
        },
    },
};
pub use crate::{message::PeerRequestSender, session::handle::PeerInfo};
use fnv::FnvHashMap;
use futures::{future::Either, io, FutureExt, StreamExt};
use reth_ecies::{stream::ECIESStream, ECIESError};
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    errors::EthStreamError,
    DisconnectReason, EthVersion, HelloMessage, Status, UnauthedEthStream, UnauthedP2PStream,
};
use reth_metrics_common::metered_sender::MeteredSender;
use reth_net_common::{
    bandwidth_meter::{BandwidthMeter, MeteredStream},
    stream::HasRemoteAddr,
};
use reth_primitives::{ForkFilter, ForkId, ForkTransition, Head, PeerId};
use reth_tasks::TaskSpawner;
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
use tracing::{instrument, trace};

mod active;
mod config;
mod handle;
pub use config::SessionsConfig;

/// Internal identifier for active sessions.
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq, Hash)]
pub struct SessionId(usize);

/// Manages a set of sessions.
#[must_use = "Session Manager must be polled to process session events."]
#[derive(Debug)]
pub(crate) struct SessionManager {
    /// Tracks the identifier for the next session.
    next_id: usize,
    /// Keeps track of all sessions
    counter: SessionCounter,
    ///  The maximum initial time an [ActiveSession] waits for a response from the peer before it
    /// responds to an _internal_ request with a `TimeoutError`
    initial_internal_request_timeout: Duration,
    /// If an [ActiveSession] does not receive a response at all within this duration then it is
    /// considered a protocol violation and the session will initiate a drop.
    protocol_breach_request_timeout: Duration,
    /// The secret key used for authenticating sessions.
    secret_key: SecretKey,
    /// The `Status` message to send to peers.
    status: Status,
    /// THe `HelloMessage` message to send to peers.
    hello_message: HelloMessage,
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
    pending_sessions: FnvHashMap<SessionId, PendingSessionHandle>,
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
    active_session_tx: MeteredSender<ActiveSessionMessage>,
    /// Receiver half that listens for [`ActiveSessionMessage`] produced by pending sessions.
    active_session_rx: ReceiverStream<ActiveSessionMessage>,
    /// Used to measure inbound & outbound bandwidth across all managed streams
    bandwidth_meter: BandwidthMeter,
}

// === impl SessionManager ===

impl SessionManager {
    /// Creates a new empty [`SessionManager`].
    pub(crate) fn new(
        secret_key: SecretKey,
        config: SessionsConfig,
        executor: Box<dyn TaskSpawner>,
        status: Status,
        hello_message: HelloMessage,
        fork_filter: ForkFilter,
        bandwidth_meter: BandwidthMeter,
    ) -> Self {
        let (pending_sessions_tx, pending_sessions_rx) = mpsc::channel(config.session_event_buffer);
        let (active_session_tx, active_session_rx) = mpsc::channel(config.session_event_buffer);

        Self {
            next_id: 0,
            counter: SessionCounter::new(config.limits),
            initial_internal_request_timeout: config.initial_internal_request_timeout,
            protocol_breach_request_timeout: config.protocol_breach_request_timeout,
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
            active_session_tx: MeteredSender::new(active_session_tx, "network_active_session"),
            active_session_rx: ReceiverStream::new(active_session_rx),
            bandwidth_meter,
        }
    }

    /// Check whether the provided [`ForkId`] is compatible based on the validation rules in
    /// `EIP-2124`.
    pub(crate) fn is_valid_fork_id(&self, fork_id: ForkId) -> bool {
        self.fork_filter.validate(fork_id).is_ok()
    }

    /// Returns the next unique [`SessionId`].
    fn next_id(&mut self) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;
        SessionId(id)
    }

    /// Returns the current status of the session.
    pub(crate) fn status(&self) -> Status {
        self.status
    }

    /// Returns the session hello message.
    pub(crate) fn hello_message(&self) -> HelloMessage {
        self.hello_message.clone()
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
    /// active [`ForkId`](ForkId). See also [`ForkFilter::set_head`].
    pub(crate) fn on_status_update(&mut self, head: Head) -> Option<ForkTransition> {
        self.status.blockhash = head.hash;
        self.status.total_difficulty = head.total_difficulty;
        self.fork_filter.set_head(head)
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
            target : "net::session",
            ?remote_addr,
            ?session_id,
            "new pending incoming session"
        );

        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let pending_events = self.pending_sessions_tx.clone();
        let metered_stream = MeteredStream::new_with_meter(stream, self.bandwidth_meter.clone());
        let secret_key = self.secret_key;
        let hello_message = self.hello_message.clone();
        let status = self.status;
        let fork_filter = self.fork_filter.clone();
        self.spawn(start_pending_incoming_session(
            disconnect_rx,
            session_id,
            metered_stream,
            pending_events,
            remote_addr,
            secret_key,
            hello_message,
            status,
            fork_filter,
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
    pub(crate) fn dial_outbound(&mut self, remote_addr: SocketAddr, remote_peer_id: PeerId) {
        let session_id = self.next_id();
        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let pending_events = self.pending_sessions_tx.clone();
        let secret_key = self.secret_key;
        let hello_message = self.hello_message.clone();
        let fork_filter = self.fork_filter.clone();
        let status = self.status;
        let band_with_meter = self.bandwidth_meter.clone();
        self.spawn(start_pending_outbound_session(
            disconnect_rx,
            pending_events,
            session_id,
            remote_addr,
            remote_peer_id,
            secret_key,
            hello_message,
            status,
            fork_filter,
            band_with_meter,
        ));

        let handle = PendingSessionHandle {
            disconnect_tx: Some(disconnect_tx),
            direction: Direction::Outgoing(remote_peer_id),
        };
        self.pending_sessions.insert(session_id, handle);
        self.counter.inc_pending_outbound();
    }

    /// Initiates a shutdown of the channel.
    ///
    /// This will trigger the disconnect on the session task to gracefully terminate. The result
    /// will be picked up by the receiver.
    pub(crate) fn disconnect(&self, node: PeerId, reason: Option<DisconnectReason>) {
        if let Some(session) = self.active_sessions.get(&node) {
            session.disconnect(reason);
        }
    }

    /// Sends a disconnect message to the peer with the given [DisconnectReason].
    pub(crate) fn disconnect_incoming_connection(
        &mut self,
        stream: TcpStream,
        reason: DisconnectReason,
    ) {
        let secret_key = self.secret_key;

        self.spawn(async move {
            if let Ok(stream) = get_eciess_stream(stream, secret_key, Direction::Incoming).await {
                let _ = UnauthedP2PStream::new(stream).send_disconnect(reason).await;
            }
        });
    }

    /// Initiates a shutdown of all sessions.
    ///
    /// It will trigger the disconnect on all the session tasks to gracefully terminate. The result
    /// will be picked by the receiver.
    pub(crate) fn disconnect_all(&self, reason: Option<DisconnectReason>) {
        for (_, session) in self.active_sessions.iter() {
            session.disconnect(reason);
        }
    }

    /// Disconnects all pending sessions.
    pub(crate) fn disconnect_all_pending(&mut self) {
        for (_, session) in self.pending_sessions.iter_mut() {
            session.disconnect();
        }
    }

    /// Sends a message to the peer's session
    pub(crate) fn send_message(&mut self, peer_id: &PeerId, msg: PeerMessage) {
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
                            target : "net::session",
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
                        trace!(target : "net::session",  ?peer_id, ?error,"closed session.");
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
                        target : "net::session",
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
                    to_session: self.active_session_tx.clone(),
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
                };

                self.spawn(session);

                let handle = ActiveSessionHandle {
                    direction,
                    session_id,
                    remote_id: peer_id,
                    version,
                    established: Instant::now(),
                    capabilities: Arc::clone(&capabilities),
                    commands_to_session,
                    client_version: client_id.clone(),
                    remote_addr,
                };

                self.active_sessions.insert(peer_id, handle);
                self.counter.inc_active(&direction);

                Poll::Ready(SessionEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version: client_id,
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
                    target : "net::session",
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
                            error: error.map(PendingSessionHandshakeError::Eth),
                        })
                    }
                    Direction::Outgoing(peer_id) => {
                        Poll::Ready(SessionEvent::OutgoingPendingSessionClosed {
                            remote_addr,
                            peer_id,
                            error: error.map(PendingSessionHandshakeError::Eth),
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
                    target : "net::session",
                    ?error,
                    ?session_id,
                    ?remote_addr,
                    ?peer_id,
                    "connection refused"
                );
                self.remove_pending_session(&session_id);
                Poll::Ready(SessionEvent::OutgoingConnectionError { remote_addr, peer_id, error })
            }
            PendingSessionEvent::EciesAuthError { remote_addr, session_id, error, direction } => {
                self.remove_pending_session(&session_id);
                trace!(
                    target : "net::session",
                    ?error,
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

    /// Returns [`PeerInfo`] for all connected peers
    pub(crate) fn get_peer_info(&self) -> Vec<PeerInfo> {
        self.active_sessions
            .values()
            .map(|session| PeerInfo {
                remote_id: session.remote_id,
                direction: session.direction,
                remote_addr: session.remote_addr,
                capabilities: session.capabilities.clone(),
                client_version: session.client_version.clone(),
            })
            .collect()
    }

    /// Returns [`PeerInfo`] for a given peer.
    ///
    /// Returns `None` if there's no active session to the peer.
    pub(crate) fn get_peer_info_by_id(&self, peer_id: PeerId) -> Option<PeerInfo> {
        self.active_sessions.get(&peer_id).map(|session| PeerInfo {
            remote_id: session.remote_id,
            direction: session.direction,
            remote_addr: session.remote_addr,
            capabilities: session.capabilities.clone(),
            client_version: session.client_version.clone(),
        })
    }
}

/// Events produced by the [`SessionManager`]
#[derive(Debug)]
pub(crate) enum SessionEvent {
    /// A new session was successfully authenticated.
    ///
    /// This session is now able to exchange data.
    SessionEstablished {
        peer_id: PeerId,
        remote_addr: SocketAddr,
        client_version: String,
        capabilities: Arc<Capabilities>,
        /// negotiated eth version
        version: EthVersion,
        status: Status,
        messages: PeerRequestSender,
        direction: Direction,
        timeout: Arc<AtomicU64>,
    },
    AlreadyConnected {
        peer_id: PeerId,
        remote_addr: SocketAddr,
        direction: Direction,
    },
    /// A session received a valid message via RLPx.
    ValidMessage {
        peer_id: PeerId,
        /// Message received from the peer.
        message: PeerMessage,
    },
    /// Received a message that does not match the announced capabilities of the peer.
    InvalidMessage {
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
        remote_addr: SocketAddr,
        error: Option<PendingSessionHandshakeError>,
    },
    /// Closed an outgoing pending session during handshaking.
    OutgoingPendingSessionClosed {
        remote_addr: SocketAddr,
        peer_id: PeerId,
        error: Option<PendingSessionHandshakeError>,
    },
    /// Failed to establish a tcp stream
    OutgoingConnectionError {
        remote_addr: SocketAddr,
        peer_id: PeerId,
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
        peer_id: PeerId,
        remote_addr: SocketAddr,
    },
}

/// Errors that can occur during handshaking/authenticating the underlying streams.
#[derive(Debug)]
pub(crate) enum PendingSessionHandshakeError {
    Eth(EthStreamError),
    Ecies(ECIESError),
}

impl PendingSessionHandshakeError {
    /// Returns the [`DisconnectReason`] if the error is a disconnect message
    pub fn as_disconnected(&self) -> Option<DisconnectReason> {
        match self {
            PendingSessionHandshakeError::Eth(eth_err) => eth_err.as_disconnected(),
            _ => None,
        }
    }
}

/// The direction of the connection.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Direction {
    /// Incoming connection.
    Incoming,
    /// Outgoing connection to a specific node.
    Outgoing(PeerId),
}

impl Direction {
    /// Returns `true` if this an incoming connection.
    pub(crate) fn is_incoming(&self) -> bool {
        matches!(self, Direction::Incoming)
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Incoming => write!(f, "incoming"),
            Direction::Outgoing(_) => write!(f, "outgoing"),
        }
    }
}

/// The error thrown when the max configured limit has been reached and no more connections are
/// accepted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Session limit reached {0}")]
pub struct ExceedsSessionLimit(pub(crate) u32);

/// Starts the authentication process for a connection initiated by a remote peer.
///
/// This will wait for the _incoming_ handshake request and answer it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_pending_incoming_session(
    disconnect_rx: oneshot::Receiver<()>,
    session_id: SessionId,
    stream: MeteredStream<TcpStream>,
    events: mpsc::Sender<PendingSessionEvent>,
    remote_addr: SocketAddr,
    secret_key: SecretKey,
    hello: HelloMessage,
    status: Status,
    fork_filter: ForkFilter,
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
    hello: HelloMessage,
    status: Status,
    fork_filter: ForkFilter,
    bandwidth_meter: BandwidthMeter,
) {
    let stream = match TcpStream::connect(remote_addr).await {
        Ok(stream) => MeteredStream::new_with_meter(stream, bandwidth_meter),
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
    )
    .await
}

/// Authenticates a session
#[allow(clippy::too_many_arguments)]
async fn authenticate(
    disconnect_rx: oneshot::Receiver<()>,
    events: mpsc::Sender<PendingSessionEvent>,
    stream: MeteredStream<TcpStream>,
    session_id: SessionId,
    remote_addr: SocketAddr,
    secret_key: SecretKey,
    direction: Direction,
    hello: HelloMessage,
    status: Status,
    fork_filter: ForkFilter,
) {
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
        direction,
        hello,
        status,
        fork_filter,
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

/// Returns an [ECIESStream] if it can be built. If not, send a
/// [PendingSessionEvent::EciesAuthError] and returns `None`
async fn get_eciess_stream<Io: AsyncRead + AsyncWrite + Unpin + HasRemoteAddr>(
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
/// On Success return the authenticated stream as [`PendingSessionEvent`]
#[allow(clippy::too_many_arguments)]
async fn authenticate_stream(
    stream: UnauthedP2PStream<ECIESStream<MeteredStream<TcpStream>>>,
    session_id: SessionId,
    remote_addr: SocketAddr,
    direction: Direction,
    hello: HelloMessage,
    status: Status,
    fork_filter: ForkFilter,
) -> PendingSessionEvent {
    // conduct the p2p handshake and return the authenticated stream
    let (p2p_stream, their_hello) = match stream.handshake(hello).await {
        Ok(stream_res) => stream_res,
        Err(err) => {
            return PendingSessionEvent::Disconnected {
                remote_addr,
                session_id,
                direction,
                error: Some(err.into()),
            }
        }
    };

    // if the hello handshake was successful we can try status handshake
    //
    // Before trying status handshake, set up the version to shared_capability
    let status = Status { version: p2p_stream.shared_capability().version(), ..status };
    let eth_unauthed = UnauthedEthStream::new(p2p_stream);
    let (eth_stream, their_status) = match eth_unauthed.handshake(status, fork_filter).await {
        Ok(stream_res) => stream_res,
        Err(err) => {
            return PendingSessionEvent::Disconnected {
                remote_addr,
                session_id,
                direction,
                error: Some(err),
            }
        }
    };
    PendingSessionEvent::Established {
        session_id,
        remote_addr,
        peer_id: their_hello.id,
        capabilities: Arc::new(Capabilities::from(their_hello.capabilities)),
        status: their_status,
        conn: eth_stream,
        direction,
        client_id: their_hello.client_version,
    }
}
