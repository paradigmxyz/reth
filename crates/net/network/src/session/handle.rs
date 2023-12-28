//! Session handles.

use crate::{
    message::PeerMessage,
    session::{conn::EthRlpxConnection, Direction, SessionId},
};
use reth_ecies::ECIESError;
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    errors::EthStreamError,
    DisconnectReason, EthVersion, Status,
};
use reth_network_api::PeerInfo;
use reth_primitives::PeerId;
use std::{io, net::SocketAddr, sync::Arc, time::Instant};
use tokio::sync::{
    mpsc::{self, error::SendError},
    oneshot,
};

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
    pub fn direction(&self) -> Direction {
        self.direction
    }
}

/// An established session with a remote peer.
///
/// Within an active session that supports the `Ethereum Wire Protocol `, three high-level tasks can
/// be performed: chain synchronization, block propagation and transaction exchange.
#[derive(Debug)]
pub struct ActiveSessionHandle {
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
    /// Sender half of the command channel used send commands _to_ the spawned session
    pub(crate) commands_to_session: mpsc::Sender<SessionCommand>,
    /// The client's name and version
    pub(crate) client_version: Arc<str>,
    /// The address we're connected to
    pub(crate) remote_addr: SocketAddr,
    /// The local address of the connection.
    pub(crate) local_addr: Option<SocketAddr>,
    /// The Status message the peer sent for the `eth` handshake
    pub(crate) status: Arc<Status>,
}

// === impl ActiveSessionHandle ===

impl ActiveSessionHandle {
    /// Sends a disconnect command to the session.
    pub fn disconnect(&self, reason: Option<DisconnectReason>) {
        // Note: we clone the sender which ensures the channel has capacity to send the message
        let _ = self.commands_to_session.clone().try_send(SessionCommand::Disconnect { reason });
    }

    /// Sends a disconnect command to the session, awaiting the command channel for available
    /// capacity.
    pub async fn try_disconnect(
        &self,
        reason: Option<DisconnectReason>,
    ) -> Result<(), SendError<SessionCommand>> {
        self.commands_to_session.clone().send(SessionCommand::Disconnect { reason }).await
    }

    /// Returns the direction of the active session (inbound or outbound).
    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// Returns the assigned session id for this session.
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Returns the negotiated eth version for this session.
    pub fn version(&self) -> EthVersion {
        self.version
    }

    /// Returns the identifier of the remote peer.
    pub fn remote_id(&self) -> PeerId {
        self.remote_id
    }

    /// Returns the timestamp when the session has been established.
    pub fn established(&self) -> Instant {
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
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Extracts the [PeerInfo] from the session handle.
    pub(crate) fn peer_info(&self) -> PeerInfo {
        PeerInfo {
            remote_id: self.remote_id,
            direction: self.direction,
            remote_addr: self.remote_addr,
            local_addr: self.local_addr,
            capabilities: self.capabilities.clone(),
            client_version: self.client_version.clone(),
            eth_version: self.version,
            status: self.status.clone(),
            session_established: self.established,
        }
    }
}

/// Events a pending session can produce.
///
/// This represents the state changes a session can undergo until it is ready to send capability messages <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md>.
///
/// A session starts with a `Handshake`, followed by a `Hello` message which
#[derive(Debug)]
pub enum PendingSessionEvent {
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
        status: Arc<Status>,
        /// The actual connection stream which can be used to send and receive `eth` protocol
        /// messages
        conn: EthRlpxConnection,
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
        error: Option<EthStreamError>,
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
pub enum SessionCommand {
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
pub enum ActiveSessionMessage {
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
    /// A session received a valid message via RLPx.
    ValidMessage {
        /// Identifier of the remote peer.
        peer_id: PeerId,
        /// Message received from the peer.
        message: PeerMessage,
    },
    /// Received a message that does not match the announced capabilities of the peer.
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
    /// Remote peer is considered in protocol violation
    ProtocolBreach {
        /// Identifier of the remote peer.
        peer_id: PeerId,
    },
}
