use crate::NodeId;
use parking_lot::Mutex;
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicUsize, Arc},
};
use tokio::sync::mpsc::UnboundedSender;

/// A _shareable_ network frontend. Used to interact with the network.
///
/// See also [`NetworkManager`](crate::NetworkManager).
#[derive(Clone)]
pub struct NetworkHandle {
    inner: Arc<NetworkInner>,
}

// === impl NetworkHandle ===

impl NetworkHandle {}

struct NetworkInner {
    /// Number of active peer sessions the node's currently handling.
    num_active_peers: Arc<AtomicUsize>,
    /// Sender half of the message channel to the [`NetworkManager`].
    to_manager_tx: UnboundedSender<NetworkHandleMessage>,
    /// The local address that accepts incoming connections.
    local_address: Arc<Mutex<SocketAddr>>,
    /// The identifier used by this node.
    local_node_id: NodeId,
    // TODO need something to access
}

/// Internal messages that can be passed to the  [`NetworkManager`](crate::NetworkManager).
pub(crate) enum NetworkHandleMessage {
    // TODO add variants for managing peers
    /// Broadcast event to announce a new block to all nodes.
    AnnounceBlock,
}
