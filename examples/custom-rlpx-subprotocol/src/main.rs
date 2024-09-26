//! Example for how to customize the network layer by adding a custom rlpx subprotocol.
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p example-custom-rlpx-subprotocol -- node
//! ```
//!
//! This launch a regular reth node with a custom rlpx subprotocol.

mod subprotocol;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use reth::builder::NodeHandle;
use reth_network::{
    config::SecretKey, protocol::IntoRlpxSubProtocol, NetworkConfig, NetworkManager,
    NetworkProtocols,
};
use reth_network_api::{test_utils::PeersHandleProvider, NetworkInfo};
use reth_node_ethereum::EthereumNode;
use subprotocol::{
    connection::CustomCommand,
    protocol::{
        event::ProtocolEvent,
        handler::{CustomRlpxProtoHandler, ProtocolState},
    },
};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _args| async move {
        // launch the node
        let NodeHandle { node, node_exit_future } =
            builder.node(EthereumNode::default()).launch().await?;
        let peer_id = node.network.peer_id();
        let peer_addr = node.network.local_addr();

        // add the custom network subprotocol to the launched node
        let (tx, mut from_peer0) = mpsc::unbounded_channel();
        let custom_rlpx_handler = CustomRlpxProtoHandler { state: ProtocolState { events: tx } };
        node.network.add_rlpx_sub_protocol(custom_rlpx_handler.into_rlpx_sub_protocol());

        // creates a separate network instance and adds the custom network subprotocol
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let (tx, mut from_peer1) = mpsc::unbounded_channel();
        let custom_rlpx_handler_2 = CustomRlpxProtoHandler { state: ProtocolState { events: tx } };
        let net_cfg = NetworkConfig::builder(secret_key)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .disable_discovery()
            .add_rlpx_sub_protocol(custom_rlpx_handler_2.into_rlpx_sub_protocol())
            .build_with_noop_provider(node.chain_spec());

        // spawn the second network instance
        let subnetwork = NetworkManager::new(net_cfg).await?;
        let subnetwork_peer_id = *subnetwork.peer_id();
        let subnetwork_peer_addr = subnetwork.local_addr();
        let subnetwork_handle = subnetwork.peers_handle();
        node.task_executor.spawn(subnetwork);

        // connect the launched node to the subnetwork
        node.network.peers_handle().add_peer(subnetwork_peer_id, subnetwork_peer_addr);

        // connect the subnetwork to the launched node
        subnetwork_handle.add_peer(*peer_id, peer_addr);

        // establish connection between peer0 and peer1
        let peer0_to_peer1 = from_peer0.recv().await.expect("peer0 connecting to peer1");
        let peer0_conn = match peer0_to_peer1 {
            ProtocolEvent::Established { direction: _, peer_id, to_connection } => {
                assert_eq!(peer_id, subnetwork_peer_id);
                to_connection
            }
        };

        // establish connection between peer1 and peer0
        let peer1_to_peer0 = from_peer1.recv().await.expect("peer1 connecting to peer0");
        let peer1_conn = match peer1_to_peer0 {
            ProtocolEvent::Established { direction: _, peer_id: peer1_id, to_connection } => {
                assert_eq!(peer1_id, *peer_id);
                to_connection
            }
        };
        info!(target:"rlpx-subprotocol", "Connection established!");

        // send a ping message from peer0 to peer1
        let (tx, rx) = oneshot::channel();
        peer0_conn.send(CustomCommand::Message { msg: "hello!".to_string(), response: tx })?;
        let response = rx.await?;
        assert_eq!(response, "hello!");
        info!(target:"rlpx-subprotocol", ?response, "New message received");

        // send a ping message from peer1 to peer0
        let (tx, rx) = oneshot::channel();
        peer1_conn.send(CustomCommand::Message { msg: "world!".to_string(), response: tx })?;
        let response = rx.await?;
        assert_eq!(response, "world!");
        info!(target:"rlpx-subprotocol", ?response, "New message received");

        info!(target:"rlpx-subprotocol", "Peers connected via custom rlpx subprotocol!");

        node_exit_future.await
    })
}
