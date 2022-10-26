//! Session handles
use tokio::sync::mpsc;

/// A handled attached to a peer session that's not authenticated yet, pending Handshake and hello message which exchanges the `capabilities` of the peer.
#[derive(Debug)]
pub struct PendingSessionHandle {

    /// Sender half of the event channel to the connection manager used to updates.
    tx: mpsc::Sender<PendingSessionEvent>,
}

/// An established session with a remote peer.
#[derive(Debug)]
pub struct ActiveSessionHandle {

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
    /// TODO add capabilities here
    Hello,
    /// Handshake unsuccessful, session was disconnected.
    Disconnected,
}