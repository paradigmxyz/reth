//! Session handles
use crate::{
    message::PeerMessage,
    session::{Direction, SessionId},
};
use reth_ecies::{stream::ECIESStream, ECIESError};
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    error::EthStreamError,
    DisconnectReason, EthStream, P2PStream, Status,
};
use reth_primitives::PeerId;
use std::{io, net::SocketAddr, sync::Arc, time::Instant};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
};

/// A handler attached to a peer session that's not authenticated yet, pending Handshake and hello
/// message which exchanges the `capabilities` of the peer.
///
/// This session needs to wait until it is authenticated.
#[derive(Debug)]
pub(crate) struct PendingSessionHandle {
    /// Can be used to tell the session to disconnect the connection/abort the handshake process.
    pub(crate) _disconnect_tx: oneshot::Sender<()>,
    /// The direction of the session
    pub(crate) direction: Direction,
}

/// An established session with a remote peer.
///
/// Within an active session that supports the `Ethereum Wire Protocol `, three high-level tasks can
/// be performed: chain synchronization, block propagation and transaction exchange.
#[derive(Debug)]
#[allow(unused)]
pub(crate) struct ActiveSessionHandle {
    /// The direction of the session
    pub(crate) direction: Direction,
    /// The assigned id for this session
    pub(crate) session_id: SessionId,
    /// The identifier of the remote peer
    pub(crate) remote_id: PeerId,
    /// The timestamp when the session has been established.
    pub(crate) established: Instant,
    /// Announced capabilities of the peer.
    pub(crate) capabilities: Arc<Capabilities>,
    /// Sender half of the command channel used send commands _to_ the spawned session
    pub(crate) commands_to_session: mpsc::Sender<SessionCommand>,
}

// === impl ActiveSessionHandle ===

impl ActiveSessionHandle {
    /// Sends a disconnect command to the session.
    pub(crate) fn disconnect(&self, reason: Option<DisconnectReason>) {
        // Note: we clone the sender which ensures the channel has capacity to send the message
        let _ = self.commands_to_session.clone().try_send(SessionCommand::Disconnect { reason });
    }
}

/// Events a pending session can produce.
///
/// This represents the state changes a session can undergo until it is ready to send capability messages <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md>.
///
/// A session starts with a `Handshake`, followed by a `Hello` message which
#[derive(Debug)]
pub(crate) enum PendingSessionEvent {
    /// Represents a successful `Hello` and `Status` exchange: <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md#hello-0x00>
    Established {
        session_id: SessionId,
        remote_addr: SocketAddr,
        /// The remote node's public key
        peer_id: PeerId,
        capabilities: Arc<Capabilities>,
        status: Status,
        conn: EthStream<P2PStream<ECIESStream<TcpStream>>>,
        direction: Direction,
    },
    /// Handshake unsuccessful, session was disconnected.
    Disconnected {
        remote_addr: SocketAddr,
        session_id: SessionId,
        direction: Direction,
        error: Option<EthStreamError>,
    },
    /// Thrown when unable to establish a [`TcpStream`].
    OutgoingConnectionError {
        remote_addr: SocketAddr,
        session_id: SessionId,
        peer_id: PeerId,
        error: io::Error,
    },
    /// Thrown when authentication via Ecies failed.
    EciesAuthError {
        remote_addr: SocketAddr,
        session_id: SessionId,
        error: ECIESError,
        direction: Direction,
    },
}

/// Commands that can be sent to the spawned session.
#[derive(Debug)]
pub(crate) enum SessionCommand {
    /// Disconnect the connection
    Disconnect {
        /// Why the disconnect was initiated
        reason: Option<DisconnectReason>,
    },
    /// Sends a message to the peer
    Message(PeerMessage),
}

/// Message variants an active session can produce and send back to the
/// [`SessionManager`](crate::session::SessionManager)
#[derive(Debug)]
pub(crate) enum ActiveSessionMessage {
    /// Session was gracefully disconnected.
    Disconnected { peer_id: PeerId, remote_addr: SocketAddr },
    /// Session was closed due an error
    ClosedOnConnectionError {
        peer_id: PeerId,
        remote_addr: SocketAddr,
        /// The error that caused the session to close
        error: EthStreamError,
    },
    /// A session received a valid message via RLPx.
    ValidMessage {
        /// Identifier of the remote peer.
        peer_id: PeerId,
        /// Message received from the peer.
        message: PeerMessage,
    },
    /// Received a message that does not match the announced capabilities of the peer.
    #[allow(unused)]
    InvalidMessage {
        /// Identifier of the remote peer.
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
}
