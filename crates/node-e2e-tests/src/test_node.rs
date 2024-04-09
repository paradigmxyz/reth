use std::sync::Arc;

use reth::{
    args::{DiscoveryArgs, NetworkArgs},
    builder::{NodeBuilder, NodeHandle},
    network::{peers::PeersHandle, NetworkEvent, NetworkEvents, PeersInfo},
    tasks::TaskExecutor,
};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{ChainSpec, NodeRecord};

use reth_tracing::tracing::info;
use tokio_stream::{wrappers::UnboundedReceiverStream, StreamExt};

/// An helper struct to handle node interactions
pub struct TestNode {
    peer_handle: PeersHandle,
    network_events: UnboundedReceiverStream<NetworkEvent>,
    pub node_record: NodeRecord,
}

impl TestNode {
    /// Creates a new test node
    pub async fn new(
        chain_spec: impl Into<Arc<ChainSpec>>,
        task_exec: TaskExecutor,
    ) -> eyre::Result<Self> {
        let network_config = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        };
        let node_config = NodeConfig::test()
            .with_chain(chain_spec)
            .with_network(network_config)
            .with_unused_ports()
            .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
            .testing_node(task_exec)
            .node(EthereumNode::default())
            .launch()
            .await?;

        let peer_handle = node.network.peers_handle().clone();
        let node_record = node.network.local_node_record();
        let network_events = node.network.event_listener();

        Ok(Self { peer_handle, network_events, node_record })
    }

    /// Adds a peer to the node and asserts the peer added event
    pub async fn add_peer(&mut self, node_record: NodeRecord) {
        self.peer_handle.add_peer(node_record.id, node_record.tcp_addr());
        match self.network_events.next().await {
            Some(NetworkEvent::PeerAdded(_)) => (),
            _ => panic!("Expected a peer added event"),
        }
    }

    /// Asserts that a session has been established
    pub async fn assert_session_established(&mut self) {
        match self.network_events.next().await {
            Some(NetworkEvent::SessionEstablished { remote_addr, .. }) => {
                info!(?remote_addr, "Session established")
            }
            _ => panic!("Expected session established event"),
        }
    }
}
