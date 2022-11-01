use reth_discv4::NodeId;

use futures::StreamExt;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::SocketAddr,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{Instant, Interval},
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A communication channel to the [`PeersManager`] to apply changes to the peer set.
pub struct PeersHandle {
    manager_tx: mpsc::UnboundedSender<PeerCommand>,
}

/// Maintains the state of _all_ the peers known to the network.
///
/// This is supposed to be owned by the network itself, but can be reached via the [`PeersHandle`].
/// From this type, connections to peers are established or disconnected, see [`PeerAction`].
///
/// The [`PeersManager`] will be notified on peer related changes
pub(crate) struct PeersManager {
    /// All peers known to the network
    peers: HashMap<NodeId, Node>,
    /// Copy of the receiver half, so new [`PeersHandle`] can be created on demand.
    manager_tx: mpsc::UnboundedSender<PeerCommand>,
    /// Receiver half of the command channel.
    handle_rx: UnboundedReceiverStream<PeerCommand>,
    /// Buffered actions until the manager is polled.
    actions: VecDeque<PeerAction>,
    /// Interval for triggering connections if there are free slots.
    refill_slots_interval: Interval,
    /// Tracks current slot stats.
    connection_info: ConnectionInfo,
}

impl PeersManager {
    /// Create a new instance with the given config
    pub(crate) fn new(config: PeersConfig) -> Self {
        let PeersConfig { refill_slots_interval, connection_info } = config;
        let (manager_tx, handle_rx) = mpsc::unbounded_channel();
        Self {
            peers: Default::default(),
            manager_tx,
            handle_rx: UnboundedReceiverStream::new(handle_rx),
            actions: Default::default(),
            refill_slots_interval: tokio::time::interval_at(
                Instant::now() + refill_slots_interval,
                refill_slots_interval,
            ),
            connection_info,
        }
    }

    /// Returns a new [`PeersHandle`] that can send commands to this type
    pub(crate) fn handle(&self) -> PeersHandle {
        PeersHandle { manager_tx: self.manager_tx.clone() }
    }

    pub(crate) fn add_discovered_node(&mut self, node: NodeId, addr: SocketAddr) {
        match self.peers.entry(node) {
            Entry::Occupied(_) => {}
            Entry::Vacant(entry) => {
                entry.insert(Node::new(addr));
            }
        }
    }

    pub(crate) fn remove_discovered_node(&mut self, _node: NodeId) {}

    /// If there's capacity for new outbound connections, this will queue new
    /// [`PeerAction::Connect`] actions.
    fn fill_outbound_slots(&mut self) {
        // This checks if there are free slots for new outbound connections available that can be
        // filled
    }

    /// Advances the state.
    ///
    /// Event hooks invoked externally may trigger a new [`PeerAction`] that are buffered until
    /// [`PeersManager::poll_next`] is called.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<PeerAction> {
        loop {
            // drain buffered actions
            if let Some(action) = self.actions.pop_front() {
                return Poll::Ready(action)
            }
            if self.refill_slots_interval.poll_tick(cx).is_ready() {
                self.fill_outbound_slots();
            }

            while let Poll::Ready(Some(_cmd)) = self.handle_rx.poll_next_unpin(cx) {
                // TODO handle incoming command
            }
        }
    }
}

/// Tracks stats about connected nodes
#[derive(Debug)]
pub struct ConnectionInfo {
    /// Currently occupied slots for outbound connections.
    num_outbound: usize,
    /// Currently occupied slots for inbound connections.
    num_inbound: usize,
    /// Maximum allowed outbound connections.
    max_outbound: usize,
    /// Maximum allowed inbound connections.
    max_inbound: usize,
}

/// Tracks info about a single node.
struct Node {
    /// Where to reach the node
    addr: SocketAddr,
    /// Reputation of the node.
    reputation: i32,
}

// === impl Node ===

impl Node {
    fn new(addr: SocketAddr) -> Self {
        Self { addr, reputation: 0 }
    }
}

/// Commands the [`PeersManager`] listens for.
pub enum PeerCommand {
    Add(NodeId),
    Remove(NodeId),
    // TODO reputation change
}

/// Actions the peer manager can trigger.
#[derive(Debug)]
pub enum PeerAction {
    /// Start a new connection to a peer.
    Connect {
        /// The peer to connect to.
        node_id: NodeId,
        /// Where to reach the node
        remote_addr: SocketAddr,
    },
    /// Disconnect an existing connection.
    Disconnect { node_id: NodeId },
}

/// Config type for initiating a [`PeersManager`] instance
#[derive(Debug)]
pub struct PeersConfig {
    /// How even to recheck free slots for outbound connections
    pub refill_slots_interval: Duration,
    /// Restrictions on connections
    pub connection_info: ConnectionInfo,
}

impl Default for PeersConfig {
    fn default() -> Self {
        Self {
            refill_slots_interval: Duration::from_millis(1_000),
            connection_info: ConnectionInfo {
                num_outbound: 0,
                num_inbound: 0,
                max_outbound: 70,
                max_inbound: 30,
            },
        }
    }
}
