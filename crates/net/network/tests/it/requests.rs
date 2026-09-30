#![allow(unreachable_pub)]
//! Tests for eth related requests

use alloy_consensus::{Header, Sealed};
use alloy_eips::NumHash;
use alloy_primitives::{BlockHash, BlockNumber, Bytes, B256};
use rand::Rng;
use reth_eth_wire::{
    BlockAccessLists, EthVersion, GetBlockAccessLists, GetReceipts, HeadersDirection,
};
use reth_ethereum_primitives::{Block, Receipt};
use reth_network::{
    eth_requests::{MAX_BLOCK_ACCESS_LISTS_SERVE, SOFT_RESPONSE_LIMIT},
    test_utils::{PeerConfig, Testnet, TestnetHandle},
    BlockDownloaderProvider, PeerRequest,
};
use reth_network_p2p::{
    bodies::client::BodiesClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersRequest},
    BlockAccessListsClient,
};
use reth_provider::{
    test_utils::MockEthProvider, BalStore, BalStoreHandle, InMemoryBalStore, ProviderError,
    ProviderResult, RawBal,
};
use reth_transaction_pool::test_utils::{TestPool, TransactionGenerator};
use std::sync::Arc;
use test_case::test_case;

type RequestsTestnet = TestnetHandle<Arc<MockEthProvider>, TestPool>;

#[test_case(None; "default")]
#[test_case(Some(EthVersion::Eth69); "eth69")]
#[tokio::test(flavor = "multi_thread")]
async fn test_get_body(version: Option<EthVersion>) {
    let (provider, net) = spawn_testnet(MockEthProvider::default(), version).await;
    let fetch0 = net.peers()[0].network().fetch_client().await.unwrap();
    let mut tx_gen = TransactionGenerator::new(rand::rng());

    // request some blocks
    for _ in 0..100 {
        // Set a new random block to the mock storage and request it via the network
        let block_hash = B256::random();
        let mut block = Block::default();
        block.body.transactions.push(tx_gen.gen_eip4844());

        provider.add_block(block_hash, block.clone());

        let res = fetch0.get_block_bodies(vec![block_hash]).await;
        assert!(res.is_ok(), "{res:?}");

        let blocks = res.unwrap().1;
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], block.body);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_body_range() {
    let (provider, net) = spawn_testnet(MockEthProvider::default(), None).await;
    let fetch0 = net.peers()[0].network().fetch_client().await.unwrap();
    let mut tx_gen = TransactionGenerator::new(rand::rng());

    let mut all_blocks = Vec::new();
    let mut block_hashes = Vec::new();
    // add some blocks
    for _ in 0..100 {
        let block_hash = B256::random();
        let mut block = Block::default();
        block.body.transactions.push(tx_gen.gen_eip4844());

        provider.add_block(block_hash, block.clone());
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

#[test_case(None; "default")]
#[test_case(Some(EthVersion::Eth69); "eth69")]
#[tokio::test(flavor = "multi_thread")]
async fn test_get_header(version: Option<EthVersion>) {
    let (provider, net) = spawn_testnet(MockEthProvider::default(), version).await;
    let fetch0 = net.peers()[0].network().fetch_client().await.unwrap();
    let mut rng = rand::rng();

    let start: u64 = rng.random();
    let mut hash = rng.random();
    // request some headers
    for idx in 0..100 {
        // Set a new random header to the mock storage and request it via the network
        let header = Header { number: start + idx, parent_hash: hash, ..Default::default() };
        hash = rng.random();

        provider.add_header(hash, header.clone());

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
    let (provider, net) = spawn_testnet(MockEthProvider::default(), None).await;
    let fetch0 = net.peers()[0].network().fetch_client().await.unwrap();
    let (start, all_headers) = add_header_chain(&provider);

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
    let (provider, net) = spawn_testnet(MockEthProvider::default(), None).await;
    let fetch0 = net.peers()[0].network().fetch_client().await.unwrap();
    let (start, all_headers) = add_header_chain(&provider);

    // ensure we can fetch the correct headers in falling direction
    // start from the last header and work backwards
    for idx in (0..100).rev() {
        // Can't fetch more than idx+1 headers when going backwards
        let count = idx + 1;
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
    let (provider, net) = spawn_testnet(MockEthProvider::default(), Some(EthVersion::Eth68)).await;
    let [peer0, peer1] = net.peers_array();

    for block_num in 1..=10 {
        let request = GetReceipts(vec![add_receipts(&provider, block_num)]);
        let receipts = peer0
            .request(*peer1.peer_id(), |response| PeerRequest::GetReceipts { request, response })
            .await
            .unwrap();

        assert_eq!(receipts.0.len(), 1);
        assert_eq!(receipts.0[0].len(), 2);
        // Eth68 receipts should have bloom filters - verify the structure
        assert_eq!(receipts.0[0][0].receipt.cumulative_gas_used, 21000);
        assert_eq!(receipts.0[0][1].receipt.cumulative_gas_used, 42000);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_get_receipts() {
    let (provider, net) = spawn_testnet(MockEthProvider::default(), Some(EthVersion::Eth69)).await;
    let [peer0, peer1] = net.peers_array();

    for block_num in 1..=10 {
        let request = GetReceipts(vec![add_receipts(&provider, block_num)]);
        let receipts = peer0
            .request(*peer1.peer_id(), |response| PeerRequest::GetReceipts69 { request, response })
            .await
            .unwrap();

        assert_eq!(receipts.0.len(), 1);
        assert_eq!(receipts.0[0].len(), 2);
        // ETH69 receipts do not include bloom filters - verify the structure
        assert_eq!(receipts.0[0][0].cumulative_gas_used, 21000);
        assert_eq!(receipts.0[0][1].cumulative_gas_used, 42000);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists() {
    let (provider, net) =
        spawn_testnet(bal_provider(InMemoryBalStore::default()), Some(EthVersion::Eth71)).await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let hash2 = B256::random();
    let bal0 = Bytes::from_static(&[0xc1, 0x01]);
    let bal2 = Bytes::from_static(&[0xc1, 0x02]);

    provider.bal_store.insert(NumHash::new(1, hash0), RawBal::from(bal0.clone())).unwrap();
    provider.bal_store.insert(NumHash::new(3, hash2), RawBal::from(bal2.clone())).unwrap();

    let response = request_block_access_lists(&net, vec![hash0, hash1, hash2]).await;
    assert_eq!(response, BlockAccessLists(vec![Some(bal0), None, Some(bal2)]));
}

// Ensures BAL responses stop at the soft response limit while keeping the item that crosses it.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_respects_response_soft_limit() {
    let (provider, net) =
        spawn_testnet(bal_provider(InMemoryBalStore::default()), Some(EthVersion::Eth71)).await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let hash2 = B256::random();
    let bal0 = raw_bal_with_len(2);
    let bal1 = raw_bal_with_len(SOFT_RESPONSE_LIMIT);
    let bal2 = raw_bal_with_len(2);
    assert!(bal0.len() + bal1.len() > SOFT_RESPONSE_LIMIT);

    provider.bal_store.insert(NumHash::new(1, hash0), RawBal::from(bal0.clone())).unwrap();
    provider.bal_store.insert(NumHash::new(2, hash1), RawBal::from(bal1.clone())).unwrap();
    provider.bal_store.insert(NumHash::new(3, hash2), RawBal::from(bal2)).unwrap();

    let response = request_block_access_lists(&net, vec![hash0, hash1, hash2]).await;

    assert_eq!(response, BlockAccessLists(vec![Some(bal0), Some(bal1)]));
}

// Ensures a single BAL larger than the soft limit is still returned.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_returns_single_oversized_bal() {
    let (provider, net) =
        spawn_testnet(bal_provider(InMemoryBalStore::default()), Some(EthVersion::Eth71)).await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let bal0 = raw_bal_with_len(SOFT_RESPONSE_LIMIT + 1);
    let bal1 = raw_bal_with_len(2);

    provider.bal_store.insert(NumHash::new(1, hash0), RawBal::from(bal0.clone())).unwrap();
    provider.bal_store.insert(NumHash::new(2, hash1), RawBal::from(bal1)).unwrap();

    let response = request_block_access_lists(&net, vec![hash0, hash1]).await;

    assert_eq!(response, BlockAccessLists(vec![Some(bal0)]));
}

// Ensures an empty BAL request roundtrips to an empty response.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_empty_request() {
    let (_, net) =
        spawn_testnet(bal_provider(InMemoryBalStore::default()), Some(EthVersion::Eth71)).await;

    let response = request_block_access_lists(&net, Vec::new()).await;

    assert_eq!(response, BlockAccessLists(Vec::new()));
}

// Ensures BAL responses are capped at MAX_BLOCK_ACCESS_LISTS_SERVE entries.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_caps_count() {
    let (provider, net) =
        spawn_testnet(bal_provider(InMemoryBalStore::default()), Some(EthVersion::Eth71)).await;

    // Request more hashes than the count cap.
    let request_count = MAX_BLOCK_ACCESS_LISTS_SERVE + 100;
    let hashes: Vec<B256> = (0..request_count).map(|_| B256::random()).collect();

    // Insert one BAL so the store isn't entirely empty (not strictly needed,
    // but keeps the test path closer to real usage).
    let bal = Bytes::from_static(&[0xc1, 0x01]);
    provider.bal_store.insert(NumHash::new(1, hashes[0]), RawBal::from(bal)).unwrap();

    let response = request_block_access_lists(&net, hashes).await;

    assert_eq!(response.0.len(), MAX_BLOCK_ACCESS_LISTS_SERVE);
}

// Ensures the fetch client can request BALs through an eth/71 peer.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_fetch_client_get_block_access_lists() {
    let (provider, net) =
        spawn_testnet(bal_provider(InMemoryBalStore::default()), Some(EthVersion::Eth71)).await;

    let hash0 = B256::random();
    let hash1 = B256::random();
    let bal0 = Bytes::from_static(&[0xc1, 0x01]);

    provider.bal_store.insert(NumHash::new(1, hash0), RawBal::from(bal0.clone())).unwrap();

    let fetch = net.peers()[0].network().fetch_client().await.unwrap();
    let response = fetch.get_block_access_lists(vec![hash0, hash1]).await.unwrap().into_data();

    assert_eq!(response, BlockAccessLists(vec![Some(bal0), None]));
}

// Ensures store errors produce a valid empty BAL response instead of synthesizing unavailable
// entries.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth71_get_block_access_lists_returns_empty_on_store_error() {
    let (_, net) =
        spawn_testnet(bal_provider(FailingLookupBalStore), Some(EthVersion::Eth71)).await;

    let response = request_block_access_lists(&net, vec![B256::random(), B256::random()]).await;

    assert_eq!(response, BlockAccessLists(Vec::new()));
}

// Ensures default fetch client BAL requests are rejected when no eth/71 peer is available.
#[tokio::test(flavor = "multi_thread")]
async fn test_eth70_fetch_client_rejects_default_block_access_lists_request() {
    let (_, net) = spawn_testnet(MockEthProvider::default(), Some(EthVersion::Eth70)).await;

    let fetch = net.peers()[0].network().fetch_client().await.unwrap();
    let err = fetch.get_block_access_lists(vec![B256::random()]).await.unwrap_err();

    assert_eq!(err, RequestError::UnsupportedCapability);
}

/// Spawns two connected peers that serve requests from `provider`, speaking only `version` if set.
async fn spawn_testnet(
    provider: MockEthProvider,
    version: Option<EthVersion>,
) -> (Arc<MockEthProvider>, RequestsTestnet) {
    reth_tracing::init_test_tracing();

    let provider = Arc::new(provider);
    let peer = || {
        let peer = PeerConfig::new(provider.clone());
        match version {
            Some(version) => peer.with_protocols([version]),
            None => peer,
        }
    };
    let net = Testnet::from_configs([peer(), peer()]).await.with_request_handlers().spawn();
    net.connect_peers().await;

    (provider, net)
}

/// Returns a provider that serves BALs from `bal_store`.
fn bal_provider(bal_store: impl BalStore) -> MockEthProvider {
    let mut provider = MockEthProvider::default();
    provider.bal_store = BalStoreHandle::new(bal_store);
    provider
}

/// Adds a chain of 100 headers with random hashes to the provider, returning the number of the
/// first header and the sealed headers.
fn add_header_chain(provider: &MockEthProvider) -> (u64, Vec<Sealed<Header>>) {
    let mut rng = rand::rng();
    let start: u64 = rng.random();
    let mut hash = rng.random();
    let headers = (0..100)
        .map(|idx| {
            let header = Header { number: start + idx, parent_hash: hash, ..Default::default() };
            hash = rng.random();
            provider.add_header(hash, header.clone());
            header.seal(hash)
        })
        .collect();
    (start, headers)
}

/// Adds a header with two receipts at `number` to the provider and returns its hash.
fn add_receipts(provider: &MockEthProvider, number: BlockNumber) -> B256 {
    let hash = B256::random();
    provider.add_header(hash, Header { number, ..Default::default() });
    provider.add_receipts(
        number,
        vec![
            Receipt { cumulative_gas_used: 21000, success: true, ..Default::default() },
            Receipt { cumulative_gas_used: 42000, success: false, ..Default::default() },
        ],
    );
    hash
}

#[derive(Debug)]
struct FailingLookupBalStore;

impl BalStore for FailingLookupBalStore {
    fn insert(&self, _num_hash: NumHash, _bal: RawBal) -> ProviderResult<()> {
        Ok(())
    }

    fn prune(&self, _tip: BlockNumber) -> ProviderResult<usize> {
        Ok(0)
    }

    fn get_by_hashes(&self, _block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        Err(ProviderError::other(std::io::Error::other("BAL lookup failed")))
    }
}

// Sends a GetBlockAccessLists request from peer 0 to peer 1.
async fn request_block_access_lists(net: &RequestsTestnet, hashes: Vec<B256>) -> BlockAccessLists {
    let [requester, responder] = net.peers_array();
    let request = GetBlockAccessLists(hashes);
    requester
        .request(*responder.peer_id(), |response| PeerRequest::GetBlockAccessLists {
            request,
            response,
        })
        .await
        .unwrap()
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
