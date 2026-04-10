//! Session handles.

use crate::{
    message::PeerMessage,
    session::{active::BroadcastItemCounter, conn::EthRlpxConnection, Direction, SessionId},
    PendingSessionHandshakeError,
};
use reth_ecies::ECIESError;
use reth_eth_wire::{
    errors::EthStreamError, Capabilities, DisconnectReason, EthVersion, NetworkPrimitives,
    UnifiedStatus,
};
use reth_network_api::PeerInfo;
use reth_network_peers::{NodeRecord, PeerId};
use reth_network_types::PeerKind;
use std::{io, net::SocketAddr, sync::Arc, time::Instant};
use tokio::sync::{
    mpsc::{self, error::SendError},
    oneshot,
};
use tracing::trace;

/// A handler attached to a peer session that's not authenticated yet, pending Handshake and hello
/// message which exchanges the `capabilities` of the peer.
///
/// This session needs to wait until it is authenticated.
#[derive(Debug)]
pub struct PendingSessionHandle {
    /// Can be used to tell the session to disconnect the connection/abort the handshake process.
    pub(crate) disconnect_tx: Option<oneshot::Sender<()>>,
    /// The direction of the session
    pub(crate) direction: Direction,
}

// === impl PendingSessionHandle ===

impl PendingSessionHandle {
    /// Sends a disconnect command to the pending session.
    pub fn disconnect(&mut self) {
        if let Some(tx) = self.disconnect_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Returns the direction of the pending session (inbound or outbound).
    pub const fn direction(&self) -> Direction {
        self.direction
    }
}

/// An established session with a remote peer.
///
/// Within an active session that supports the `Ethereum Wire Protocol`, three high-level tasks can
/// be performed: chain synchronization, block propagation and transaction exchange.
#[derive(Debug)]
pub struct ActiveSessionHandle<N: NetworkPrimitives> {
    /// The direction of the session
    pub(crate) direction: Direction,
    /// The assigned id for this session
    pub(crate) session_id: SessionId,
    /// negotiated eth version
    pub(crate) version: EthVersion,
    /// The identifier of the remote peer
    pub(crate) remote_id: PeerId,
    /// The timestamp when the session has been established.
    pub(crate) established: Instant,
    /// Announced capabilities of the peer.
    pub(crate) capabilities: Arc<Capabilities>,
    /// Sender for commands to the spawned session with broadcast-aware backpressure.
    pub(crate) commands: SessionCommandSender<N>,
    /// The client's name and version
    pub(crate) client_version: Arc<str>,
    /// The address we're connected to
    pub(crate) remote_addr: SocketAddr,
    /// The local address of the connection.
    pub(crate) local_addr: Option<SocketAddr>,
    /// The Status message the peer sent for the `eth` handshake
    pub(crate) status: Arc<UnifiedStatus>,
}

// === impl ActiveSessionHandle ===

impl<N: NetworkPrimitives> ActiveSessionHandle<N> {
    /// Sends a disconnect command to the session.
    pub fn disconnect(&self, reason: Option<DisconnectReason>) {
        self.commands.disconnect(reason);
    }

    /// Sends a disconnect command to the session via the unbounded channel.
    pub fn try_disconnect(
        &self,
        reason: Option<DisconnectReason>,
    ) -> Result<(), SendError<SessionCommand<N>>> {
        self.commands.try_disconnect(reason)
    }

    /// Returns the direction of the active session (inbound or outbound).
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    /// Returns the assigned session id for this session.
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Returns the negotiated eth version for this session.
    pub const fn version(&self) -> EthVersion {
        self.version
    }

    /// Returns the identifier of the remote peer.
    pub const fn remote_id(&self) -> PeerId {
        self.remote_id
    }

    /// Returns the timestamp when the session has been established.
    pub const fn established(&self) -> Instant {
        self.established
    }

    /// Returns the announced capabilities of the peer.
    pub fn capabilities(&self) -> Arc<Capabilities> {
        self.capabilities.clone()
    }

    /// Returns the client's name and version.
    pub fn client_version(&self) -> Arc<str> {
        self.client_version.clone()
    }

    /// Returns the address we're connected to.
    pub const fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Returns the current number of in-flight broadcast items.
    pub fn queued_broadcast_items(&self) -> usize {
        self.commands.queued_broadcast_items()
    }

    /// Extracts the [`PeerInfo`] from the session handle.
    pub(crate) fn peer_info(&self, record: &NodeRecord, kind: PeerKind) -> PeerInfo {
        PeerInfo {
            remote_id: self.remote_id,
            direction: self.direction,
            enode: record.to_string(),
            enr: None,
            remote_addr: self.remote_addr,
            local_addr: self.local_addr,
            capabilities: self.capabilities.clone(),
            client_version: self.client_version.clone(),
            eth_version: self.version,
            status: self.status.clone(),
            session_established: self.established,
            kind,
        }
    }
}

/// Sender half of the session command channel with broadcast-aware backpressure.
///
/// Commands are first sent through a bounded channel. If the bounded channel is full and the
/// message is a broadcast with room under the broadcast item limit, it overflows to a dedicated
/// unbounded channel that the session task drains alongside the bounded one.
///
/// The shared `broadcast_items` counter tracks items across **all** buffers (bounded channel,
/// overflow channel, and the session's outgoing queue), so the
/// [`SessionManager`](super::SessionManager) has an accurate view of total in-flight broadcast
/// pressure.
#[derive(Debug)]
pub(crate) struct SessionCommandSender<N: NetworkPrimitives> {
    /// Bounded channel for all commands (primary path).
    tx: mpsc::Sender<SessionCommand<N>>,
    /// Unbounded channel used for broadcasts that overflow the bounded channel, and for
    /// disconnect commands (which must never be dropped due to backpressure).
    unbounded_tx: mpsc::UnboundedSender<SessionCommand<N>>,
    /// Shared counter of in-flight broadcast items (channels + outgoing queue).
    broadcast_items: BroadcastItemCounter,
}

impl<N: NetworkPrimitives> SessionCommandSender<N> {
    /// Creates a new sender with the given bounded channel, unbounded channel, and shared counter.
    pub(crate) const fn new(
        tx: mpsc::Sender<SessionCommand<N>>,
        unbounded_tx: mpsc::UnboundedSender<SessionCommand<N>>,
        broadcast_items: BroadcastItemCounter,
    ) -> Self {
        Self { tx, unbounded_tx, broadcast_items }
    }

    /// Sends a disconnect command via the unbounded channel so it is never dropped due to
    /// backpressure.
    pub(crate) fn disconnect(&self, reason: Option<DisconnectReason>) {
        let _ = self.unbounded_tx.send(SessionCommand::Disconnect { reason });
    }

    /// Sends a disconnect command via the unbounded channel.
    ///
    /// This is infallible from a capacity standpoint (unbounded), but will fail if the
    /// receiver has been dropped (session closed).
    pub(crate) fn try_disconnect(
        &self,
        reason: Option<DisconnectReason>,
    ) -> Result<(), SendError<SessionCommand<N>>> {
        self.unbounded_tx.send(SessionCommand::Disconnect { reason }).map_err(|e| SendError(e.0))
    }

    /// Sends a message to the session with broadcast-aware backpressure.
    ///
    /// For broadcast messages, the broadcast item counter is incremented **before** the message
    /// enters any channel, ensuring the counter always reflects the true in-flight count.
    /// If the bounded channel is full, broadcasts overflow to the unbounded channel (up to the
    /// item limit). Non-broadcast messages that cannot fit in the bounded channel are dropped.
    ///
    /// Returns `true` if the message was accepted, `false` if it was dropped.
    pub(crate) fn send_message(&self, msg: PeerMessage<N>) -> bool {
        if msg.is_broadcast() {
            let items = msg.message_item_count();

            // Check + increment atomically (optimistic)
            if !self.broadcast_items.try_add(items) {
                return false;
            }

            // Try bounded channel first
            match self.tx.try_send(SessionCommand::Message(msg)) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(cmd)) => {
                    // Overflow to unbounded channel (counter already incremented)
                    let _ = self.unbounded_tx.send(cmd);
                    true
                }
                Err(_) => {
                    // Channel closed, undo increment
                    self.broadcast_items.sub(items);
                    false
                }
            }
        } else {
            // Non-broadcast: bounded channel only
            match self.tx.try_send(SessionCommand::Message(msg)) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(SessionCommand::Message(msg))) => {
                    trace!(
                        target: "net::session",
                        msg_kind = msg.message_kind(),
                        "session command buffer full, dropping non-broadcast message"
                    );
                    false
                }
                Err(_) => false,
            }
        }
    }

    /// Returns the current number of in-flight broadcast items.
    pub(crate) fn queued_broadcast_items(&self) -> usize {
        self.broadcast_items.get()
    }
}

/// Events a pending session can produce.
///
/// This represents the state changes a session can undergo until it is ready to send capability messages <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md>.
///
/// A session starts with a `Handshake`, followed by a `Hello` message which
#[derive(Debug)]
pub enum PendingSessionEvent<N: NetworkPrimitives> {
    /// Represents a successful `Hello` and `Status` exchange: <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md#hello-0x00>
    Established {
        /// An internal identifier for the established session
        session_id: SessionId,
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The local address of the connection
        local_addr: Option<SocketAddr>,
        /// The remote node's public key
        peer_id: PeerId,
        /// All capabilities the peer announced
        capabilities: Arc<Capabilities>,
        /// The Status message the peer sent for the `eth` handshake
        status: Arc<UnifiedStatus>,
        /// The actual connection stream which can be used to send and receive `eth` protocol
        /// messages
        conn: EthRlpxConnection<N>,
        /// The direction of the session, either `Inbound` or `Outgoing`
        direction: Direction,
        /// The remote node's user agent, usually containing the client name and version
        client_id: String,
    },
    /// Handshake unsuccessful, session was disconnected.
    Disconnected {
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The internal identifier for the disconnected session
        session_id: SessionId,
        /// The direction of the session, either `Inbound` or `Outgoing`
        direction: Direction,
        /// The error that caused the disconnect
        error: Option<PendingSessionHandshakeError>,
    },
    /// Thrown when unable to establish a [`TcpStream`](tokio::net::TcpStream).
    OutgoingConnectionError {
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The internal identifier for the disconnected session
        session_id: SessionId,
        /// The remote node's public key
        peer_id: PeerId,
        /// The error that caused the outgoing connection failure
        error: io::Error,
    },
    /// Thrown when authentication via ECIES failed.
    EciesAuthError {
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The internal identifier for the disconnected session
        session_id: SessionId,
        /// The error that caused the ECIES session to fail
        error: ECIESError,
        /// The direction of the session, either `Inbound` or `Outgoing`
        direction: Direction,
    },
}

/// Commands that can be sent to the spawned session.
#[derive(Debug)]
pub enum SessionCommand<N: NetworkPrimitives> {
    /// Disconnect the connection
    Disconnect {
        /// Why the disconnect was initiated
        reason: Option<DisconnectReason>,
    },
    /// Sends a message to the peer
    Message(PeerMessage<N>),
}

/// Message variants an active session can produce and send back to the
/// [`SessionManager`](crate::session::SessionManager)
#[derive(Debug)]
pub enum ActiveSessionMessage<N: NetworkPrimitives> {
    /// Session was gracefully disconnected.
    Disconnected {
        /// The remote node's public key
        peer_id: PeerId,
        /// The remote node's socket address
        remote_addr: SocketAddr,
    },
    /// Session was closed due an error
    ClosedOnConnectionError {
        /// The remote node's public key
        peer_id: PeerId,
        /// The remote node's socket address
        remote_addr: SocketAddr,
        /// The error that caused the session to close
        error: EthStreamError,
    },
    /// A session received a valid message via `RLPx`.
    ValidMessage {
        /// Identifier of the remote peer.
        peer_id: PeerId,
        /// Message received from the peer.
        message: PeerMessage<N>,
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
}
