use alloy_primitives::B256;
use futures::StreamExt;
use reth_network::{test_utils::Testnet, NetworkEventListenerProvider, Peers};
use reth_network_api::{
    events::{NetworkEvent, PeerEvent},
    test_utils::PeersHandleProvider,
};
use reth_provider::test_utils::MockEthProvider;
use reth_ress_protocol::{ExecutionWitness, GetHeaders, NodeType};
use reth_zk_ress_protocol::{
    test_utils::{MockRessProtocolProvider, NoopZkRessProtocolProvider},
    ProtocolEvent, ProtocolState, ZkRessPeerRequest, ZkRessProtocolHandler,
};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_on_stateful_pair() {
    reth_tracing::init_test_tracing();
    let mut net = Testnet::create_with(2, MockEthProvider::default()).await;
    let protocol_provider = NoopZkRessProtocolProvider::<ExecutionWitness>::default();

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    let peer0 = &mut net.peers_mut()[0];
    peer0.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider.clone(),
        node_type: NodeType::Stateful,
        peers_handle: peer0.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    let (tx, mut from_peer1) = mpsc::unbounded_channel();
    let peer1 = &mut net.peers_mut()[1];
    peer1.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider,
        node_type: NodeType::Stateful,
        peers_handle: peer1.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    // spawn and connect all the peers
    let handle = net.spawn();
    handle.connect_peers().await;

    match from_peer0.recv().await.unwrap() {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[1].peer_id());
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };
    match from_peer1.recv().await.unwrap() {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };

    let mut peer0_event_listener = handle.peers()[0].network().event_listener();
    loop {
        if let NetworkEvent::Peer(PeerEvent::SessionClosed { peer_id, .. }) =
            peer0_event_listener.next().await.unwrap()
        {
            assert_eq!(peer_id, *handle.peers()[1].peer_id());
            break
        }
    }

    let mut peer1_event_listener = handle.peers()[1].network().event_listener();
    loop {
        if let NetworkEvent::Peer(PeerEvent::SessionClosed { peer_id, .. }) =
            peer1_event_listener.next().await.unwrap()
        {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
            break
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn message_exchange() {
    reth_tracing::init_test_tracing();
    let mut net = Testnet::create_with(2, MockEthProvider::default()).await;
    let protocol_provider = NoopZkRessProtocolProvider::<ExecutionWitness>::default();

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    let peer0 = &mut net.peers_mut()[0];
    peer0.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider.clone(),
        node_type: NodeType::Stateless,
        peers_handle: peer0.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    let (tx, mut from_peer1) = mpsc::unbounded_channel();
    let peer1 = &mut net.peers_mut()[1];
    peer1.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider,
        node_type: NodeType::Stateless,
        peers_handle: peer1.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    // spawn and connect all the peers
    let handle = net.spawn();
    handle.connect_peers().await;

    let peer0_to_peer1 = from_peer0.recv().await.unwrap();
    let peer0_conn = match peer0_to_peer1 {
        ProtocolEvent::Established { direction: _, peer_id, to_connection } => {
            assert_eq!(peer_id, *handle.peers()[1].peer_id());
            to_connection
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };

    let peer1_to_peer0 = from_peer1.recv().await.unwrap();
    match peer1_to_peer0 {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };

    // send get headers message from peer0 to peer1
    let (tx, rx) = oneshot::channel();
    peer0_conn
        .send(ZkRessPeerRequest::GetHeaders {
            request: GetHeaders { start_hash: B256::ZERO, limit: 1 },
            tx,
        })
        .unwrap();
    assert_eq!(rx.await.unwrap(), Vec::new());

    // send get bodies message from peer0 to peer1
    let (tx, rx) = oneshot::channel();
    peer0_conn.send(ZkRessPeerRequest::GetBlockBodies { request: Vec::new(), tx }).unwrap();
    assert_eq!(rx.await.unwrap(), Vec::new());

    // send get proof message from peer0 to peer1
    let (tx, rx) = oneshot::channel();
    peer0_conn.send(ZkRessPeerRequest::GetProof { block_hash: B256::ZERO, tx }).unwrap();
    assert_eq!(rx.await.unwrap(), Default::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn proof_fetching_does_not_block() {
    reth_tracing::init_test_tracing();
    let mut net = Testnet::create_with(2, MockEthProvider::default()).await;

    let proof_delay = Duration::from_millis(100);
    let protocol_provider =
        MockRessProtocolProvider::<ExecutionWitness>::default().with_proof_delay(proof_delay);

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    let peer0 = &mut net.peers_mut()[0];
    peer0.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider.clone(),
        node_type: NodeType::Stateless,
        peers_handle: peer0.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    let (tx, mut from_peer1) = mpsc::unbounded_channel();
    let peer1 = &mut net.peers_mut()[1];
    peer1.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider,
        node_type: NodeType::Stateless,
        peers_handle: peer1.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    // spawn and connect all the peers
    let handle = net.spawn();
    handle.connect_peers().await;

    let peer0_to_peer1 = from_peer0.recv().await.unwrap();
    let peer0_conn = match peer0_to_peer1 {
        ProtocolEvent::Established { direction: _, peer_id, to_connection } => {
            assert_eq!(peer_id, *handle.peers()[1].peer_id());
            to_connection
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };

    let peer1_to_peer0 = from_peer1.recv().await.unwrap();
    match peer1_to_peer0 {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };

    // send get proof message from peer0 to peer1
    let proof_requested_at = Instant::now();
    let (proof_tx, proof_rx) = oneshot::channel();
    peer0_conn.send(ZkRessPeerRequest::GetProof { block_hash: B256::ZERO, tx: proof_tx }).unwrap();

    // send get header request from peer0 to peer1
    let headers_requested_at = Instant::now();
    let (tx, rx) = oneshot::channel();
    peer0_conn
        .send(ZkRessPeerRequest::GetHeaders {
            request: GetHeaders { start_hash: B256::ZERO, limit: 1 },
            tx,
        })
        .unwrap();
    assert_eq!(rx.await.unwrap(), Vec::new());
    assert!(headers_requested_at.elapsed() < proof_delay);

    // await for proof response
    assert_eq!(proof_rx.await.unwrap(), Default::default());
    assert!(proof_requested_at.elapsed() >= proof_delay);
}

#[tokio::test(flavor = "multi_thread")]
async fn max_active_connections() {
    reth_tracing::init_test_tracing();
    let mut net = Testnet::create_with(3, MockEthProvider::default()).await;
    let protocol_provider = NoopZkRessProtocolProvider::<ExecutionWitness>::default();

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    let peer0 = &mut net.peers_mut()[0];
    peer0.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider.clone(),
        node_type: NodeType::Stateful,
        peers_handle: peer0.handle().peers_handle().clone(),
        max_active_connections: 1,
        state: ProtocolState::new(tx),
    });

    let (tx, _from_peer1) = mpsc::unbounded_channel();
    let peer1 = &mut net.peers_mut()[1];
    let peer1_id = peer1.peer_id();
    let peer1_addr = peer1.local_addr();
    peer1.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider.clone(),
        node_type: NodeType::Stateless,
        peers_handle: peer1.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    let (tx, _from_peer2) = mpsc::unbounded_channel();
    let peer2 = &mut net.peers_mut()[2];
    let peer2_id = peer2.peer_id();
    let peer2_addr = peer2.local_addr();
    peer2.add_rlpx_sub_protocol(ZkRessProtocolHandler {
        protocol_name: "zkress",
        protocol_version: 1,
        provider: protocol_provider,
        node_type: NodeType::Stateless,
        peers_handle: peer2.handle().peers_handle().clone(),
        max_active_connections: 100,
        state: ProtocolState::new(tx),
    });

    let handle = net.spawn();

    // connect peers 0 and 1
    let peer0_handle = &handle.peers()[0];
    peer0_handle.network().add_peer(peer1_id, peer1_addr);

    let _peer0_to_peer1 = match from_peer0.recv().await.unwrap() {
        ProtocolEvent::Established { peer_id, to_connection, .. } => {
            assert_eq!(peer_id, *peer1_id);
            to_connection
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };

    // connect peers 0 and 2, max active connections exceeded.
    peer0_handle.network().add_peer(peer2_id, peer2_addr);
    match from_peer0.recv().await.unwrap() {
        ProtocolEvent::MaxActiveConnectionsExceeded { num_active } => {
            assert_eq!(num_active, 1);
        }
        ev => {
            panic!("unexpected event: {ev:?}");
        }
    };
}
