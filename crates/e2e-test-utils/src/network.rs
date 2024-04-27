use std::marker::PhantomData;

use crate::node::NodeTestContext;
use futures_util::StreamExt;
use reth::network::{NetworkEvent, NetworkEvents, NetworkHandle, PeersInfo};
use reth_node_builder::FullNodeComponents;
use reth_primitives::NodeRecord;
use reth_tokio_util::EventStream;
use reth_tracing::tracing::info;

/// Helper for network operations
pub struct NetworkTestContext<N: FullNodeComponents> {
    network_events: EventStream<NetworkEvent>,
    network: NetworkHandle,
    _phantom: PhantomData<N>,
}

impl<N: FullNodeComponents> NetworkTestContext<N> {
    /// Creates a new network helper
    pub fn new(network: NetworkHandle) -> Self {
        let network_events = network.event_listener();
        Self { network_events, network, _phantom: PhantomData }
    }

    /// Connects to a network node and expects a session from both peers
    pub async fn connect(&mut self, node: &mut NodeTestContext<N>) {
        self.add_peer(node.network.record()).await;
        node.network.add_peer(self.record()).await;
        node.network.expect_session().await;
        self.expect_session().await;
    }

    /// Adds a peer to the network node via network handle
    pub async fn add_peer(&mut self, node_record: NodeRecord) {
        self.network.peers_handle().add_peer(node_record.id, node_record.tcp_addr());

        match self.network_events.next().await {
            Some(NetworkEvent::PeerAdded(_)) => (),
            _ => panic!("Expected a peer added event"),
        }
    }

    /// Returns the network node record
    pub fn record(&self) -> NodeRecord {
        self.network.local_node_record()
    }

    /// Expects a session to be established
    pub async fn expect_session(&mut self) {
        match self.network_events.next().await {
            Some(NetworkEvent::SessionEstablished { remote_addr, .. }) => {
                info!(?remote_addr, "Session established")
            }
            _ => panic!("Expected session established event"),
        }
    }
}
