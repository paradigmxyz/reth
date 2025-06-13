#![allow(unreachable_pub)]
//! Tests for eth related requests

use alloy_consensus::Header;
use rand::Rng;
use reth_eth_wire::{EthVersion, HeadersDirection};
use reth_ethereum_primitives::Block;
use reth_network::{
    test_utils::{NetworkEventStream, PeerConfig, Testnet},
    BlockDownloaderProvider, NetworkEventListenerProvider,
};
use reth_network_api::{NetworkInfo, Peers};
use reth_network_p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersRequest},
};
use reth_provider::test_utils::MockEthProvider;
use reth_transaction_pool::test_utils::{TestPool, TransactionGenerator};
use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread")]
async fn test_get_body() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::rng();
    let mock_provider = Arc::new(MockEthProvider::default());
    let mut tx_gen = TransactionGenerator::new(rand::rng());

    let mut net = Testnet::create_with(2, mock_provider.clone()).await;

    // install request handlers
    net.for_each_mut(|peer| peer.install_request_handler());

    let handle0 = net.peers()[0].handle();
    let mut events0 = NetworkEventStream::new(handle0.event_listener());

    let handle1 = net.peers()[1].handle();

    let _handle = net.spawn();

    let fetch0 = handle0.fetch_client().await.unwrap();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let connected = events0.next_session_established().await.unwrap();
    assert_eq!(connected, *handle1.peer_id());

    // request some blocks
    for _ in 0..100 {
        // Set a new random block to the mock storage and request it via the network
        let block_hash = rng.random();
        let mut block: Block = Block::default();
        block.body.transactions.push(tx_gen.gen_eip4844());

        mock_provider.add_block(block_hash, block.clone());

        let res = fetch0.get_block_bodies(vec![block_hash]).await;
        assert!(res.is_ok(), "{res:?}");

        let blocks = res.unwrap().1;
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], block.body);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_header() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::rng();
    let mock_provider = Arc::new(MockEthProvider::default());

    let mut net = Testnet::create_with(2, mock_provider.clone()).await;

    // install request handlers
    net.for_each_mut(|peer| peer.install_request_handler());

    let handle0 = net.peers()[0].handle();
    let mut events0 = NetworkEventStream::new(handle0.event_listener());

    let handle1 = net.peers()[1].handle();

    let _handle = net.spawn();

    let fetch0 = handle0.fetch_client().await.unwrap();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let connected = events0.next_session_established().await.unwrap();
    assert_eq!(connected, *handle1.peer_id());

    let start: u64 = rng.random();
    let mut hash = rng.random();
    // request some headers
    for idx in 0..100 {
        // Set a new random header to the mock storage and request it via the network
        let header = Header { number: start + idx, parent_hash: hash, ..Default::default() };
        hash = rng.random();

        mock_provider.add_header(hash, header.clone());

        let req =
            HeadersRequest { start: hash.into(), limit: 1, direction: HeadersDirection::Falling };

        let res = fetch0.get_headers(req).await;
        assert!(res.is_ok(), "{res:?}");

        let headers = res.unwrap().1;
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0], header);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth68_get_receipts() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::rng();
    let mock_provider = Arc::new(MockEthProvider::default());

    let mut net: Testnet<Arc<MockEthProvider>, TestPool> = Testnet::default();

    // Create peers with ETH68 protocol explicitly
    let p0 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth68.into()));
    net.add_peer_with_config(p0).await.unwrap();

    let p1 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth68.into()));
    net.add_peer_with_config(p1).await.unwrap();

    // install request handlers
    net.for_each_mut(|peer| peer.install_request_handler());

    let handle0 = net.peers()[0].handle();
    let mut events0 = NetworkEventStream::new(handle0.event_listener());

    let handle1 = net.peers()[1].handle();

    let _handle = net.spawn();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let connected = events0.next_session_established().await.unwrap();
    assert_eq!(connected, *handle1.peer_id());

    // Create test receipts and add them to the mock provider
    for block_num in 1..=10 {
        let block_hash = rng.random();
        let header = Header { number: block_num, ..Default::default() };

        // Create some test receipts
        let receipts = vec![
            reth_ethereum_primitives::Receipt {
                cumulative_gas_used: 21000,
                success: true,
                ..Default::default()
            },
            reth_ethereum_primitives::Receipt {
                cumulative_gas_used: 42000,
                success: false,
                ..Default::default()
            },
        ];

        mock_provider.add_header(block_hash, header.clone());
        mock_provider.add_receipts(header.number, receipts);

        // Test receipt request via low-level peer request
        let (tx, rx) = oneshot::channel();
        handle0.send_request(
            *handle1.peer_id(),
            reth_network::PeerRequest::GetReceipts {
                request: reth_eth_wire::GetReceipts(vec![block_hash]),
                response: tx,
            },
        );

        let result = rx.await.unwrap();
        let receipts_response = result.unwrap();
        assert_eq!(receipts_response.0.len(), 1);
        assert_eq!(receipts_response.0[0].len(), 2);
        // Eth68 receipts should have bloom filters - verify the structure
        assert_eq!(receipts_response.0[0][0].receipt.cumulative_gas_used, 21000);
        assert_eq!(receipts_response.0[0][1].receipt.cumulative_gas_used, 42000);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_get_headers() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::rng();
    let mock_provider = Arc::new(MockEthProvider::default());

    let mut net: Testnet<Arc<MockEthProvider>, TestPool> = Testnet::default();

    // Create peers with ETH69 protocol
    let p0 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p0).await.unwrap();

    let p1 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p1).await.unwrap();

    // install request handlers
    net.for_each_mut(|peer| peer.install_request_handler());

    let handle0 = net.peers()[0].handle();
    let mut events0 = NetworkEventStream::new(handle0.event_listener());

    let handle1 = net.peers()[1].handle();

    let _handle = net.spawn();

    let fetch0 = handle0.fetch_client().await.unwrap();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let connected = events0.next_session_established().await.unwrap();
    assert_eq!(connected, *handle1.peer_id());

    let start: u64 = rng.random();
    let mut hash = rng.random();
    // request some headers via eth69 connection
    for idx in 0..50 {
        let header = Header { number: start + idx, parent_hash: hash, ..Default::default() };
        hash = rng.random();

        mock_provider.add_header(hash, header.clone());

        let req =
            HeadersRequest { start: hash.into(), limit: 1, direction: HeadersDirection::Falling };

        let res = fetch0.get_headers(req).await;
        assert!(res.is_ok(), "{res:?}");

        let headers = res.unwrap().1;
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0], header);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_get_bodies() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::rng();
    let mock_provider = Arc::new(MockEthProvider::default());
    let mut tx_gen = TransactionGenerator::new(rand::rng());

    let mut net: Testnet<Arc<MockEthProvider>, TestPool> = Testnet::default();

    // Create peers with ETH69 protocol
    let p0 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p0).await.unwrap();

    let p1 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p1).await.unwrap();

    // install request handlers
    net.for_each_mut(|peer| peer.install_request_handler());

    let handle0 = net.peers()[0].handle();
    let mut events0 = NetworkEventStream::new(handle0.event_listener());

    let handle1 = net.peers()[1].handle();

    let _handle = net.spawn();

    let fetch0 = handle0.fetch_client().await.unwrap();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let connected = events0.next_session_established().await.unwrap();
    assert_eq!(connected, *handle1.peer_id());

    // request some blocks via eth69 connection
    for _ in 0..50 {
        let block_hash = rng.random();
        let mut block: Block = Block::default();
        block.body.transactions.push(tx_gen.gen_eip4844());

        mock_provider.add_block(block_hash, block.clone());

        let res = fetch0.get_block_bodies(vec![block_hash]).await;
        assert!(res.is_ok(), "{res:?}");

        let blocks = res.unwrap().1;
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], block.body);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_get_receipts() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::rng();
    let mock_provider = Arc::new(MockEthProvider::default());

    let mut net: Testnet<Arc<MockEthProvider>, TestPool> = Testnet::default();

    // Create peers with ETH69 protocol
    let p0 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p0).await.unwrap();

    let p1 = PeerConfig::with_protocols(mock_provider.clone(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p1).await.unwrap();

    // install request handlers
    net.for_each_mut(|peer| peer.install_request_handler());

    let handle0 = net.peers()[0].handle();
    let mut events0 = NetworkEventStream::new(handle0.event_listener());

    let handle1 = net.peers()[1].handle();

    let _handle = net.spawn();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    // Wait for the session to be established
    let connected = events0.next_session_established().await.unwrap();
    assert_eq!(connected, *handle1.peer_id());

    // Create test receipts and add them to the mock provider
    for block_num in 1..=10 {
        let block_hash = rng.random();
        let header = Header { number: block_num, ..Default::default() };

        // Create some test receipts
        let receipts = vec![
            reth_ethereum_primitives::Receipt {
                cumulative_gas_used: 21000,
                success: true,
                ..Default::default()
            },
            reth_ethereum_primitives::Receipt {
                cumulative_gas_used: 42000,
                success: false,
                ..Default::default()
            },
        ];

        mock_provider.add_header(block_hash, header.clone());
        mock_provider.add_receipts(header.number, receipts);

        let (tx, rx) = oneshot::channel();
        handle0.send_request(
            *handle1.peer_id(),
            reth_network::PeerRequest::GetReceipts {
                request: reth_eth_wire::GetReceipts(vec![block_hash]),
                response: tx,
            },
        );

        let result = rx.await.unwrap();
        let receipts_response = match result {
            Ok(resp) => resp,
            Err(e) => panic!("Failed to get receipts response: {e:?}"),
        };
        assert_eq!(receipts_response.0.len(), 1);
        assert_eq!(receipts_response.0[0].len(), 2);
        // When using GetReceipts request with ETH69 peers, the response should still include bloom
        // filters The protocol version handling is done at a lower level
        assert_eq!(receipts_response.0[0][0].receipt.cumulative_gas_used, 21000);
        assert_eq!(receipts_response.0[0][1].receipt.cumulative_gas_used, 42000);
    }
}
