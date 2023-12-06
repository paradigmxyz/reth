//! Tests for eth related requests

use rand::Rng;
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersRequest},
};
use reth_network::{
    test_utils::{NetworkEventStream, Testnet},
    NetworkEvents,
};
use reth_network_api::{NetworkInfo, Peers};
use reth_primitives::{
    Block, BlockBody, Bytes, Header, HeadersDirection, Signature, Transaction, TransactionKind,
    TransactionSigned, TxEip2930, U256,
};
use reth_provider::test_utils::MockEthProvider;
use std::sync::Arc;

/// Returns a new [`TransactionSigned`] with some random parameters
pub fn rng_transaction(rng: &mut impl rand::RngCore) -> TransactionSigned {
    let request = Transaction::Eip2930(TxEip2930 {
        chain_id: rng.gen(),
        nonce: rng.gen(),
        gas_price: rng.gen(),
        gas_limit: rng.gen(),
        to: TransactionKind::Create,
        value: rng.gen::<u128>().into(),
        input: Bytes::from(vec![1, 2]),
        access_list: Default::default(),
    });
    let signature = Signature { odd_y_parity: true, r: U256::default(), s: U256::default() };

    TransactionSigned::from_transaction_and_signature(request, signature)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_body() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::thread_rng();
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

    // request some blocks
    for _ in 0..100 {
        // Set a new random block to the mock storage and request it via the network
        let block_hash = rng.gen();
        let mut block = Block::default();
        block.body.push(rng_transaction(&mut rng));

        mock_provider.add_block(block_hash, block.clone());

        let res = fetch0.get_block_bodies(vec![block_hash]).await;
        assert!(res.is_ok(), "{res:?}");

        let blocks = res.unwrap().1;
        assert_eq!(blocks.len(), 1);
        let expected =
            BlockBody { transactions: block.body, ommers: block.ommers, withdrawals: None };
        assert_eq!(blocks[0], expected);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_header() {
    reth_tracing::init_test_tracing();
    let mut rng = rand::thread_rng();
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

    let start: u64 = rng.gen();
    let mut hash = rng.gen();
    // request some headers
    for idx in 0..100 {
        // Set a new random header to the mock storage and request it via the network
        let header = Header { number: start + idx, parent_hash: hash, ..Default::default() };
        hash = rng.gen();

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
