use reth_discv4::NodeId;

use futures::StreamExt;
use std::{
    collections::{HashMap, VecDeque},
    task::{Context, Poll},
};
use tokio::{sync::mpsc, time::Interval};
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
    peers: HashMap<NodeId, Peer>,
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
    /// If there's capacity for new outbound connections, this will queue new
    /// [`PeerAction::Connect`] actions
    fn fill_outbound_slots(&mut self) {
        // This checks if there are free slots for new outbound connections available that can be
        // filled
    }

    /// Advances the state.
    ///
    /// Event hooks invoked externally may trigger a new [`PeerAction`] that are buffered until
    /// [`PeersManager::poll_next`] is called.
    pub fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<PeerAction> {
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
struct ConnectionInfo {
    /// Currently occupied slots for outbound connections.
    num_outbound: usize,
    /// Currently occupied slots for inbound connections.
    num_inbound: usize,
    /// Maximum allowed outbound connections.
    max_outbound: usize,
    /// Maximum allowed inbound connections.
    max_inbound: usize,
}

/// Tracks stats about a single peer.
struct Peer {
    // TODO needs reputation etc..
}

/// Commands the [`PeersManager`] listens for.
pub enum PeerCommand {
    Add(NodeId),
    Remove(NodeId),
    // TODO reputation change
}

#[derive(Debug)]
pub enum PeerAction {
    /// Start a new connection to a peer.
    Connect {
        /// The peer to connect to.
        peer: NodeId,
    },
    /// Disconnect an existing connection.
    Disconnect { peer: NodeId },
}
