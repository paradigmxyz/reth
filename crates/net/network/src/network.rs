use crate::{manager::NetworkEvent, peers::PeersHandle, NodeId};
use parking_lot::Mutex;
use reth_primitives::{H256, U256};
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicUsize, Arc},
};
use tokio::sync::{mpsc, mpsc::UnboundedSender};

/// A _shareable_ network frontend. Used to interact with the network.
///
/// See also [`NetworkManager`](crate::NetworkManager).
#[derive(Clone)]
pub struct NetworkHandle {
    /// The Arc'ed delegate that contains the state.
    inner: Arc<NetworkInner>,
}

// === impl NetworkHandle ===

impl NetworkHandle {
    /// Creates a single new instance.
    pub(crate) fn new(
        num_active_peers: Arc<AtomicUsize>,
        listener_address: Arc<Mutex<SocketAddr>>,
        to_manager_tx: UnboundedSender<NetworkHandleMessage>,
        local_node_id: NodeId,
        peers: PeersHandle,
    ) -> Self {
        let inner = NetworkInner {
            num_active_peers,
            to_manager_tx,
            listener_address,
            local_node_id,
            peers,
        };
        Self { inner: Arc::new(inner) }
    }

    fn manager(&self) -> &UnboundedSender<NetworkHandleMessage> {
        &self.inner.to_manager_tx
    }

    /// Creates a new [`NetworkEvent`] listener channel.
    pub fn event_listener(&self) -> mpsc::UnboundedReceiver<NetworkEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.manager().send(NetworkHandleMessage::EventListener(tx));
        rx
    }
}

struct NetworkInner {
    /// Number of active peer sessions the node's currently handling.
    num_active_peers: Arc<AtomicUsize>,
    /// Sender half of the message channel to the [`NetworkManager`].
    to_manager_tx: UnboundedSender<NetworkHandleMessage>,
    /// The local address that accepts incoming connections.
    listener_address: Arc<Mutex<SocketAddr>>,
    /// The identifier used by this node.
    local_node_id: NodeId,
    /// Access to the all the nodes
    peers: PeersHandle, // TODO need something to access
}

/// Internal messages that can be passed to the  [`NetworkManager`](crate::NetworkManager).
pub(crate) enum NetworkHandleMessage {
    /// Add a new listener for [`NetworkEvent`].
    EventListener(UnboundedSender<NetworkEvent>),
    /// Broadcast event to announce a new block to all nodes.
    AnnounceBlock,
    /// Returns the newest imported block by the network.
    NewestBlock(H256, U256),
}
