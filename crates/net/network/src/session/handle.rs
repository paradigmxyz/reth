//! Session handles
use crate::{
    capability::{Capabilities, CapabilityMessage},
    session::SessionId,
    NodeId,
};
use std::{net::SocketAddr, sync::Arc, time::Instant};
use tokio::sync::{mpsc, oneshot};

/// A handler attached to a peer session that's not authenticated yet, pending Handshake and hello
/// message which exchanges the `capabilities` of the peer.
///
/// This session needs to wait until it is authenticated.
#[derive(Debug)]
pub struct PendingSessionHandle {
    /// Can be used to tell the session to disconnect the connection/abort the handshake process.
    pub(crate) disconnect_tx: oneshot::Sender<()>,
}

/// An established session with a remote peer.
///
/// Within an active session that supports the `Ethereum Wire Protocol `, three high-level tasks can
/// be performed: chain synchronization, block propagation and transaction exchange.
#[derive(Debug)]
pub struct ActiveSessionHandle {
    /// The assigned id for this session
    pub(crate) session_id: SessionId,
    /// The identifier of the remote peer
    pub(crate) remote_id: NodeId,
    /// The timestamp when the session has been established.
    pub(crate) established: Instant,
    /// Announced capabilities of the peer.
    pub(crate) capabilities: Arc<Capabilities>,
    /// Sender half of the command channel used send commands _to_ the spawned session
    pub(crate) commands: mpsc::Sender<SessionCommand>,
}

/// Events a pending session can produce.
///
/// This represents the state changes a session can undergo until it is ready to send capability messages <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md>.
///
/// A session starts with a `Handshake`, followed by a `Hello` message which
#[derive(Debug)]
pub enum PendingSessionEvent {
    /// Initial handshake step was successful <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md#initial-handshake>
    SuccessfulHandshake { remote_addr: SocketAddr, session_id: SessionId },
    /// Represents a successful `Hello` exchange: <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md#hello-0x00>
    Hello { session_id: SessionId, node_id: NodeId, capabilities: Arc<Capabilities>, stream: () },
    /// Handshake unsuccessful, session was disconnected.
    Disconnected { remote_addr: SocketAddr, session_id: SessionId },
}

/// Commands that can be sent to the spawned session.
#[derive(Debug)]
pub(crate) enum SessionCommand {
    Disconnect,
}

/// Message variants an active session can produce and send back to the
/// [`SessionManager`](crate::session::SessionManager)
#[derive(Debug)]
pub enum ActiveSessionMessage {
    /// Session disconnected.
    Closed { node_id: NodeId, remote_addr: SocketAddr },
    /// A session received a valid message via RLPx.
    ValidMessage {
        node_id: NodeId,
        /// Message received from the peer.
        message: CapabilityMessage,
    },
    /// Received a message that does not match the announced capabilities of the peer.
    InvalidMessage {
        node_id: NodeId,
        capabilities: Arc<Capabilities>,
        /// Message received from the peer.
        message: CapabilityMessage,
    },
}

/// A Cloneable connection for sending messages directly to the session of a peer.
#[derive(Debug, Clone)]
pub struct PeerMessageSender {
    /// id of the remote node.
    pub(crate) peer: NodeId,
    /// The Sender half connected to a session.
    pub(crate) to_session_tx: mpsc::Sender<CapabilityMessage>,
}
