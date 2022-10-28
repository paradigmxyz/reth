//! Capability messaging
//!
//! An RLPx stream is multiplexed via the prepended message-id of a framed message.
//! Capabilities are exchanged via the RLPx `Hello` message as pairs of `(id, version)`, <https://github.com/ethereum/devp2p/blob/master/rlpx.md#capability-messaging>

use crate::NodeId;
use bytes::Bytes;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Represents all capabilities of a node.
#[derive(Debug, Clone, Eq, PartialEq)]
// TODO replace with SmolStr
pub struct Capabilities(pub HashMap<String, usize>);

/// A Capability message consisting of the message-id and the payload
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CapabilityMessage {
    pub id: usize,
    pub payload: Bytes,
}

/// A Cloneable connection for sending messages directly to the session of a peer.
#[derive(Debug, Clone)]
pub struct PeerMessageSender {
    /// id of the remote node.
    peer: NodeId,
    /// The Sender half connected to a session.
    to_session_tx: mpsc::Sender<CapabilityMessage>,
}
