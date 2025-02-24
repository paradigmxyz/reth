use alloy_primitives::{Bytes, B256};
use futures::StreamExt;
use reth_network::{test_utils::Testnet, NetworkEventListenerProvider};
use reth_network_api::{
    events::{NetworkEvent, PeerEvent},
    test_utils::PeersHandleProvider,
};
use reth_network_ress::{
    test_utils::{MockRessProtocolProvider, NoopRessProtocolProvider},
    GetHeaders, NodeType, ProtocolEvent, ProtocolState, RessPeerRequest, RessProtocolHandler,
};
use reth_provider::test_utils::MockEthProvider;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_on_stateful_pair() {
    reth_tracing::init_test_tracing();
    let mut net = Testnet::create_with(2, MockEthProvider::default()).await;
    let protocol_provider = NoopRessProtocolProvider;

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    let peer0 = &mut net.peers_mut()[0];
    peer0.add_rlpx_sub_protocol(RessProtocolHandler {
        provider: protocol_provider,
        node_type: NodeType::Stateful,
        peers_handle: peer0.handle().peers_handle().clone(),
        state: ProtocolState { events_sender: tx },
    });

    let (tx, mut from_peer1) = mpsc::unbounded_channel();
    let peer1 = &mut net.peers_mut()[1];
    peer1.add_rlpx_sub_protocol(RessProtocolHandler {
        provider: protocol_provider,
        node_type: NodeType::Stateful,
        peers_handle: peer1.handle().peers_handle().clone(),
        state: ProtocolState { events_sender: tx },
    });

    // spawn and connect all the peers
    let handle = net.spawn();
    handle.connect_peers().await;

    match from_peer0.recv().await.unwrap() {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[1].peer_id());
        }
    };
    match from_peer1.recv().await.unwrap() {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
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
    let protocol_provider = NoopRessProtocolProvider;

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    let peer0 = &mut net.peers_mut()[0];
    peer0.add_rlpx_sub_protocol(RessProtocolHandler {
        provider: protocol_provider,
        node_type: NodeType::Stateless,
        peers_handle: peer0.handle().peers_handle().clone(),
        state: ProtocolState { events_sender: tx },
    });

    let (tx, mut from_peer1) = mpsc::unbounded_channel();
    let peer1 = &mut net.peers_mut()[1];
    peer1.add_rlpx_sub_protocol(RessProtocolHandler {
        provider: protocol_provider,
        node_type: NodeType::Stateless,
        peers_handle: peer1.handle().peers_handle().clone(),
        state: ProtocolState { events_sender: tx },
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
    };

    let peer1_to_peer0 = from_peer1.recv().await.unwrap();
    match peer1_to_peer0 {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
        }
    };

    // send get headers message from peer0 to peer1
    let (tx, rx) = oneshot::channel();
    peer0_conn
        .send(RessPeerRequest::GetHeaders {
            request: GetHeaders { start_hash: B256::ZERO, limit: 1 },
            tx,
        })
        .unwrap();
    assert_eq!(rx.await.unwrap(), Vec::new());

    // send get bodies message from peer0 to peer1
    let (tx, rx) = oneshot::channel();
    peer0_conn.send(RessPeerRequest::GetBlockBodies { request: Vec::new(), tx }).unwrap();
    assert_eq!(rx.await.unwrap(), Vec::new());

    // send get witness message from peer0 to peer1
    let (tx, rx) = oneshot::channel();
    peer0_conn.send(RessPeerRequest::GetWitness { block_hash: B256::ZERO, tx }).unwrap();
    assert_eq!(rx.await.unwrap(), Default::default());

    // send get bytecode message from peer0 to peer1
    let (tx, rx) = oneshot::channel();
    peer0_conn.send(RessPeerRequest::GetBytecode { code_hash: B256::ZERO, tx }).unwrap();
    assert_eq!(rx.await.unwrap(), Bytes::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn witness_fetching_does_not_block() {
    reth_tracing::init_test_tracing();
    let mut net = Testnet::create_with(2, MockEthProvider::default()).await;

    let witness_delay = Duration::from_millis(100);
    let protocol_provider = MockRessProtocolProvider::default().with_witness_delay(witness_delay);

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    let peer0 = &mut net.peers_mut()[0];
    peer0.add_rlpx_sub_protocol(RessProtocolHandler {
        provider: protocol_provider.clone(),
        node_type: NodeType::Stateless,
        peers_handle: peer0.handle().peers_handle().clone(),
        state: ProtocolState { events_sender: tx },
    });

    let (tx, mut from_peer1) = mpsc::unbounded_channel();
    let peer1 = &mut net.peers_mut()[1];
    peer1.add_rlpx_sub_protocol(RessProtocolHandler {
        provider: protocol_provider,
        node_type: NodeType::Stateless,
        peers_handle: peer1.handle().peers_handle().clone(),
        state: ProtocolState { events_sender: tx },
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
    };

    let peer1_to_peer0 = from_peer1.recv().await.unwrap();
    match peer1_to_peer0 {
        ProtocolEvent::Established { peer_id, .. } => {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
        }
    };

    // send get witness message from peer0 to peer1
    let witness_requested_at = Instant::now();
    let (witness_tx, witness_rx) = oneshot::channel();
    peer0_conn
        .send(RessPeerRequest::GetWitness { block_hash: B256::ZERO, tx: witness_tx })
        .unwrap();

    // send get bytecode message from peer0 to peer1
    let bytecode_requested_at = Instant::now();
    let (tx, rx) = oneshot::channel();
    peer0_conn.send(RessPeerRequest::GetBytecode { code_hash: B256::ZERO, tx }).unwrap();
    assert_eq!(rx.await.unwrap(), Bytes::default());
    assert!(bytecode_requested_at.elapsed() < witness_delay);

    // await for witness response
    assert_eq!(witness_rx.await.unwrap(), Default::default());
    assert!(witness_requested_at.elapsed() >= witness_delay);
}
