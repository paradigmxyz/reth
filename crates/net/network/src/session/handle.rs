//! Session handles
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

// TODO: integrate with the actual P2PStream

/// A handler attached to a peer session that's not authenticated yet, pending Handshake and hello
/// message which exchanges the `capabilities` of the peer.
///
/// This session needs to wait until it is authenticated.
#[derive(Debug)]
pub struct PendingSessionHandle {
    /// The sender half that can be used as soon as the session is active, See
    /// [`ActiveSessionHandle`]
    // TODO: depending on how the transition from pending -> authenticated will be implemented this
    // could be removed if on authentication the stream itself is returned and a new session is
    // spawned.
    _tx: mpsc::Sender<SessionCommand>,
    /// Can be used to tell the session to disconnect the connection/abort the handshake process.
    disconnect: oneshot::Sender<()>,
}

/// An established session with a remote peer.
///
/// Within an active session that supports the `Ethereum Wire Protocol `, three high-level tasks can
/// be performed: chain synchronization, block propagation and transaction exchange.
#[derive(Debug)]
pub struct ActiveSessionHandle {
    /// The timestamp when the session has been established.
    established: Instant,
    /// Sender half of the command channel used send commands _to_ the spawned session
    tx: mpsc::Sender<SessionCommand>,
}

/// Events a pending session can produce.
///
/// This represents the state changes a session can undergo until it is ready to send capability messages <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md>.
///
/// A session starts with a `Handshake`, followed by a `Hello` message which
#[derive(Debug)]
pub enum PendingSessionEvent {
    /// Initial handshake step was successful <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md#initial-handshake>
    SuccessfulHandshake,
    /// Represents a successful `Hello` exchange: <https://github.com/ethereum/devp2p/blob/6b0abc3d956a626c28dce1307ee9f546db17b6bd/rlpx.md#hello-0x00>
    /// TODO this could return the authenticated stream
    Hello,
    /// Handshake unsuccessful, session was disconnected.
    Disconnected,
}

/// Commands that can be sent to the spawned session.
#[derive(Debug)]
pub(crate) enum SessionCommand {
    // TODO this should have all the command variants that a spawned session can execute, such as
    // ethereum wire protocol messages This should include variants for Block propagation,
    // transaction exchange, state sync
}

/// Message variants an active session can produce and send back to the
/// [`SessionManager`](crate::session::SessionManager)
#[derive(Debug)]
pub enum ActiveSessionMessage {
    // TODO: this is the inverse of [`SessionCommand`] that represents requests received from the
    // remote peer.
    /// Session terminated.
    Closed,
}
