#![allow(unreachable_pub)]
//! Tests for eth related requests

use alloy_consensus::Header;
use alloy_primitives::{Bytes, B256};
use rand::Rng;
use reth_eth_wire::{BlockAccessLists, EthVersion, GetBlockAccessLists, HeadersDirection};
use reth_ethereum_primitives::Block;
use reth_network::{
    eth_requests::{MAX_BLOCK_ACCESS_LISTS_SERVE, SOFT_RESPONSE_LIMIT},
    test_utils::{NetworkEventStream, PeerConfig, Testnet, TestnetHandle},
    BlockDownloaderProvider, NetworkEventListenerProvider,
};
use reth_network_api::{NetworkInfo, Peers};
use reth_network_p2p::{
    bodies::client::BodiesClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersRequest},
    BalRequirement, BlockAccessListsClient,
};
use reth_provider::{test_utils::MockEthProvider, BalStoreHandle, InMemoryBalStore};
use reth_transaction_pool::test_utils::{TestPool, TransactionGenerator};
use std::sync::Arc;
use tokio::sync::oneshot;

type BalTestnetHandle = TestnetHandle<Arc<MockEthProvider>, TestPool>;

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
async fn test_get_body_range() {
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

    let mut all_blocks = Vec::new();
    let mut block_hashes = Vec::new();
    // add some blocks
    for _ in 0..100 {
        let block_hash = rng.random();
        let mut block: Block = Block::default();
        block.body.transactions.push(tx_gen.gen_eip4844());

        mock_provider.add_block(block_hash, block.clone());
        all_blocks.push(block);
        block_hashes.push(block_hash);
    }

    // ensure we can fetch the correct bodies
    for idx in 0..100 {
        let count = std::cmp::min(100 - idx, 10); // Limit to 10 bodies per request
        let hashes_to_fetch = &block_hashes[idx..idx + count];

        let res = fetch0.get_block_bodies(hashes_to_fetch.to_vec()).await;
        assert!(res.is_ok(), "{res:?}");

        let bodies = res.unwrap().1;
        assert_eq!(bodies.len(), count);
        for i in 0..bodies.len() {
            assert_eq!(bodies[i], all_blocks[idx + i].body);
        }
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
async fn test_get_header_range() {
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
    let mut all_headers = Vec::new();
    // add some headers
    for idx in 0..100 {
        // Set a new random header to the mock storage and request it via the network
        let header = Header { number: start + idx, parent_hash: hash, ..Default::default() };
        hash = rng.random();
        mock_provider.add_header(hash, header.clone());
        all_headers.push(header.seal(hash));
    }

    // ensure we can fetch the correct headers
    for idx in 0..100 {
        let count = 100 - idx;
        let header = &all_headers[idx];
        let req = HeadersRequest {
            start: header.hash().into(),
            limit: count as u64,
            direction: HeadersDirection::Rising,
        };

        let res = fetch0.get_headers(req).await;
        assert!(res.is_ok(), "{res:?}");

        let headers = res.unwrap().1;
        assert_eq!(headers.len(), count);
        assert_eq!(headers[0].number, start + idx as u64);
        for i in 0..headers.len() {
            assert_eq!(&headers[i], all_headers[idx + i].inner());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_header_range_falling() {
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
    let mut all_headers = Vec::new();
    // add some headers
    for idx in 0..100 {
        // Set a new random header to the mock storage
        let header = Header { number: start + idx, parent_hash: hash, ..Default::default() };
        hash = rng.random();
        mock_provider.add_header(hash, header.clone());
        all_headers.push(header.seal(hash));
    }

    // ensure we can fetch the correct headers in falling direction
    // start from the last header and work backwards
    for idx in (0..100).rev() {
        let count = std::cmp::min(idx + 1, 100); // Can't fetch more than idx+1 headers when going backwards
        let header = &all_headers[idx];
        let req = HeadersRequest {
            start: header.hash().into(),
            limit: count as u64,
            direction: HeadersDirection::Falling,
        };

        let res = fetch0.get_headers(req).await;
        assert!(res.is_ok(), "{res:?}");

        let headers = res.unwrap().1;
        assert_eq!(headers.len(), count);
        assert_eq!(headers[0].number, start + idx as u64);
        // When fetching in Falling direction, headers come in reverse order
        for i in 0..headers.len() {
            assert_eq!(&headers[i], all_headers[idx - i].inner());
        }
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
            reth_network::PeerRequest::GetReceipts69 {
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
        // ETH69 receipts do not include bloom filters - verify the structure
        assert_eq!(receipts_response.0[0][0].cumulative_gas_used, 21000);
        assert_eq!(receipts_response.0[0][1].cumulative_gas_used, 42000);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists() {
    reth_tracing::init_test_tracing();
    let (net, bal_store) = spawn_eth71_bal_testnet().await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let hash2 = B256::random();
    let bal0 = Bytes::from_static(&[0xc1, 0x01]);
    let bal2 = Bytes::from_static(&[0xc1, 0x02]);

    bal_store.insert(hash0, 1, bal0.clone()).unwrap();
    bal_store.insert(hash2, 3, bal2.clone()).unwrap();

    let response = request_block_access_lists(&net, vec![hash0, hash1, hash2]).await;
    assert_eq!(
        response,
        BlockAccessLists(vec![bal0, Bytes::from_static(&[alloy_rlp::EMPTY_LIST_CODE]), bal2,])
    );
}

// Ensures BAL responses stop at the soft response limit while keeping the item that crosses it.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_respects_response_soft_limit() {
    reth_tracing::init_test_tracing();
    let (net, bal_store) = spawn_eth71_bal_testnet().await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let hash2 = B256::random();
    let bal0 = raw_bal_with_len(2);
    let bal1 = raw_bal_with_len(SOFT_RESPONSE_LIMIT);
    let bal2 = raw_bal_with_len(2);
    assert!(bal0.len() + bal1.len() > SOFT_RESPONSE_LIMIT);

    bal_store.insert(hash0, 1, bal0.clone()).unwrap();
    bal_store.insert(hash1, 2, bal1.clone()).unwrap();
    bal_store.insert(hash2, 3, bal2).unwrap();

    let response = request_block_access_lists(&net, vec![hash0, hash1, hash2]).await;

    assert_eq!(response, BlockAccessLists(vec![bal0, bal1]));
}

// Ensures a single BAL larger than the soft limit is still returned.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_returns_single_oversized_bal() {
    reth_tracing::init_test_tracing();
    let (net, bal_store) = spawn_eth71_bal_testnet().await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let bal0 = raw_bal_with_len(SOFT_RESPONSE_LIMIT + 1);
    let bal1 = raw_bal_with_len(2);

    bal_store.insert(hash0, 1, bal0.clone()).unwrap();
    bal_store.insert(hash1, 2, bal1).unwrap();

    let response = request_block_access_lists(&net, vec![hash0, hash1]).await;

    assert_eq!(response, BlockAccessLists(vec![bal0]));
}

// Ensures an empty BAL request roundtrips to an empty response.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_empty_request() {
    reth_tracing::init_test_tracing();
    let (net, _) = spawn_eth71_bal_testnet().await;

    let response = request_block_access_lists(&net, Vec::new()).await;

    assert_eq!(response, BlockAccessLists(Vec::new()));
}

// Ensures BAL responses are capped at MAX_BLOCK_ACCESS_LISTS_SERVE entries.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_caps_count() {
    reth_tracing::init_test_tracing();
    let (net, bal_store) = spawn_eth71_bal_testnet().await;

    // Request more hashes than the count cap.
    let request_count = MAX_BLOCK_ACCESS_LISTS_SERVE + 100;
    let hashes: Vec<B256> = (0..request_count).map(|_| B256::random()).collect();

    // Insert one BAL so the store isn't entirely empty (not strictly needed,
    // but keeps the test path closer to real usage).
    let bal = Bytes::from_static(&[0xc1, 0x01]);
    bal_store.insert(hashes[0], 1, bal).unwrap();

    let response = request_block_access_lists(&net, hashes).await;

    assert_eq!(response.0.len(), MAX_BLOCK_ACCESS_LISTS_SERVE);
}

// Ensures the fetch client can request BALs through an eth/71 peer.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_fetch_client_get_block_access_lists() {
    reth_tracing::init_test_tracing();
    let (net, bal_store) = spawn_eth71_bal_testnet().await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let bal0 = Bytes::from_static(&[0xc1, 0x01]);

    bal_store.insert(hash0, 1, bal0.clone()).unwrap();

    let fetch = net.peers()[0].network().fetch_client().await.unwrap();
    let response = fetch.get_block_access_lists(vec![hash0, hash1]).await.unwrap().into_data();

    assert_eq!(
        response,
        BlockAccessLists(vec![bal0, Bytes::from_static(&[alloy_rlp::EMPTY_LIST_CODE])])
    );
}

// Ensures fetch client BAL requests are rejected when no eth/71 peer is available.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth70_fetch_client_rejects_optional_block_access_lists_request() {
    reth_tracing::init_test_tracing();
    let (net, _) = spawn_bal_testnet([EthVersion::Eth70, EthVersion::Eth70]).await;

    let fetch = net.peers()[0].network().fetch_client().await.unwrap();
    let err = fetch
        .get_block_access_lists_with_requirement(vec![B256::random()], BalRequirement::Optional)
        .await
        .unwrap_err();

    assert_eq!(err, RequestError::UnsupportedCapability);
}

async fn spawn_eth71_bal_testnet() -> (BalTestnetHandle, BalStoreHandle) {
    spawn_bal_testnet([EthVersion::Eth71, EthVersion::Eth71]).await
}

// Spawns a BAL testnet with one peer per requested eth protocol version.
async fn spawn_bal_testnet(
    versions: impl IntoIterator<Item = EthVersion>,
) -> (BalTestnetHandle, BalStoreHandle) {
    let mut mock_provider = MockEthProvider::default();
    let bal_store = BalStoreHandle::new(InMemoryBalStore::default());
    mock_provider.bal_store = bal_store.clone();
    let mock_provider = Arc::new(mock_provider);

    let mut net: Testnet<Arc<MockEthProvider>, TestPool> = Testnet::default();

    for version in versions {
        let peer = PeerConfig::with_protocols(mock_provider.clone(), Some(version.into()));
        net.add_peer_with_config(peer).await.unwrap();
    }

    net.for_each_mut(|peer| peer.install_request_handler());

    let net = net.spawn();
    net.connect_peers().await;

    (net, bal_store)
}

// Sends a GetBlockAccessLists request from peer 0 to peer 1.
async fn request_block_access_lists(net: &BalTestnetHandle, hashes: Vec<B256>) -> BlockAccessLists {
    let requester = &net.peers()[0];
    let responder = &net.peers()[1];
    let (tx, rx) = oneshot::channel();

    requester.network().send_request(
        *responder.peer_id(),
        reth_network::PeerRequest::GetBlockAccessLists {
            request: GetBlockAccessLists(hashes),
            response: tx,
        },
    );

    rx.await.unwrap().unwrap()
}

// Builds a complete raw RLP list item with the requested encoded byte length.
fn raw_bal_with_len(len: usize) -> Bytes {
    assert!(len > 0);

    let mut payload_length = len - 1;
    loop {
        let header_length = alloy_rlp::Header { list: true, payload_length }.length();
        let next_payload_length = len.checked_sub(header_length).unwrap();
        if next_payload_length == payload_length {
            break
        }
        payload_length = next_payload_length;
    }

    let mut out = Vec::with_capacity(len);
    alloy_rlp::Header { list: true, payload_length }.encode(&mut out);
    out.resize(len, alloy_rlp::EMPTY_LIST_CODE);
    Bytes::from(out)
}
