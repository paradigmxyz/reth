//! Eth/72 interop tests.
//!
//! A raw peer speaking the go-ethereum eth/72 wire format connects to a node and exercises the
//! announcement cell mask in both directions as well as a blob-elided `PooledTransactions`
//! response per [EIP-8070](https://eips.ethereum.org/EIPS/eip-8070).

use alloy_consensus::{
    constants::EIP4844_TX_TYPE_ID, transaction::TxEip4844WithSidecar, Header, SignableTransaction,
    TxEip4844,
};
use alloy_eips::{
    eip2718::Encodable2718,
    eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant},
};
use alloy_primitives::{Address, B256, U256};
use futures::{SinkExt, StreamExt};
use reth_chainspec::MAINNET;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    EthMessage, EthNetworkPrimitives, EthStream, EthVersion, HelloMessageWithProtocols,
    NewPooledTransactionHashes72, P2PStream, StatusBuilder, UnauthedEthStream, UnauthedP2PStream,
};
use reth_eth_wire_types::{message::RequestPair, PooledTransactions};
use reth_ethereum_forks::EthereumHardfork;
use reth_ethereum_primitives::{Block, PooledTransactionVariant};
use reth_network::{
    test_utils::{NetworkEventStream, PeerConfig, Testnet},
    transactions::config::{TransactionPropagationMode, TransactionsManagerConfig},
    NetworkEventListenerProvider, PeersInfo,
};
use reth_network_peers::pk2id;
use reth_primitives_traits::{crypto::secp256k1::sign_message, SignerRecoverable};
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{
    test_utils::TransactionGenerator, AddedTransactionOutcome, PoolTransaction, TransactionPool,
};
use secp256k1::{SecretKey, SECP256K1};
use std::time::Duration;
use tokio::net::TcpStream;

type RawEthStream = EthStream<P2PStream<ECIESStream<TcpStream>>, EthNetworkPrimitives>;

/// Returns the next eth message from the raw peer's stream, skipping unrelated traffic such as
/// block range updates.
async fn next_message(stream: &mut RawEthStream) -> EthMessage<EthNetworkPrimitives> {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("timed out awaiting eth message")
            .expect("stream terminated")
            .expect("stream errored");
        if !matches!(message, EthMessage::BlockRangeUpdate(_)) {
            return message
        }
    }
}

/// A signed EIP-4844 pooled transaction in the eth/72 shape: the sidecar keeps commitments and
/// cell proof metadata while the blob payloads are elided (fetched separately via `GetCells`).
fn blob_tx_without_blobs() -> PooledTransactionVariant {
    let mut versioned_hash = B256::random();
    versioned_hash.0[0] = 0x01;

    let tx = TxEip4844 {
        chain_id: 1,
        nonce: 0,
        gas_limit: 100_000,
        max_fee_per_gas: 20_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        to: Address::random(),
        value: U256::ZERO,
        access_list: Default::default(),
        blob_versioned_hashes: vec![versioned_hash],
        max_fee_per_blob_gas: 20_000_000_000,
        input: Default::default(),
    };

    let signature = sign_message(B256::random(), tx.signature_hash()).unwrap();
    let sidecar = BlobTransactionSidecarVariant::Eip7594(BlobTransactionSidecarEip7594 {
        blobs: vec![],
        commitments: vec![Default::default()],
        cell_proofs: vec![],
    });

    PooledTransactionVariant::Eip4844(
        TxEip4844WithSidecar::from_tx_and_sidecar(tx, sidecar).into_signed(signature),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth72_blob_announcements_and_elided_pooled_response() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();

    // A recent tip so the pool validator activates the blob fork checks; the network status
    // itself stays at the genesis head.
    let tip = Header {
        number: 1,
        parent_hash: MAINNET.genesis_hash(),
        timestamp: 1_750_000_000,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(7),
        excess_blob_gas: Some(0),
        blob_gas_used: Some(0),
        ..Default::default()
    };
    provider.add_block(tip.hash_slow(), Block { header: tip, body: Default::default() });

    let mut net = Testnet::create_with(0, provider.clone()).await;
    net.add_peer_with_config(PeerConfig::with_protocols(
        provider.clone(),
        Some(EthVersion::Eth72.into()),
    ))
    .await
    .unwrap();

    // Announce hashes to every peer instead of broadcasting transactions in full, so the raw
    // peer receives `NewPooledTransactionHashes` for non-blob transactions as well.
    let tx_manager_config = TransactionsManagerConfig {
        propagation_mode: TransactionPropagationMode::Max(0),
        ..Default::default()
    };
    let net = net.with_eth_pool_config(tx_manager_config);
    let handle = net.spawn();

    let node = &handle.peers()[0];
    let node_id = *node.peer_id();
    let node_pool = node.pool().unwrap();
    let mut events = NetworkEventStream::new(node.network().event_listener());

    // connect a raw eth/72 peer
    let raw_key = SecretKey::new(&mut rand_08::thread_rng());
    let raw_id = pk2id(&raw_key.public_key(SECP256K1));

    let tcp = TcpStream::connect(node.local_addr()).await.unwrap();
    let ecies = ECIESStream::connect(tcp, raw_key, node_id).await.unwrap();
    let hello = HelloMessageWithProtocols::builder(raw_id)
        .protocols(vec![EthVersion::Eth72.into()])
        .build();
    let (p2p, _their_hello) = UnauthedP2PStream::new(ecies).handshake(hello).await.unwrap();
    let version = p2p.shared_capabilities().eth_version().unwrap();
    assert_eq!(version, EthVersion::Eth72);

    let mut status = StatusBuilder::default().build();
    status.set_eth_version(version);
    let fork_filter = MAINNET.hardfork_fork_filter(EthereumHardfork::Frontier).unwrap();
    let (mut eth_stream, _their_status) = UnauthedEthStream::new(p2p)
        .handshake::<EthNetworkPrimitives>(status, fork_filter)
        .await
        .unwrap();

    assert_eq!(events.next_session_established().await.unwrap(), raw_id);
    let baseline_reputation = node.peer_handle().peer_by_id(raw_id).await.unwrap().reputation();

    // 1) node -> raw peer: a plain pending transaction is announced with the eth/72 message
    let mut tx_gen = TransactionGenerator::new(rand::rng());
    let tx = tx_gen.gen_eip1559_pooled();
    provider.add_account(tx.sender(), ExtendedAccount::new(0, U256::from(100_000_000u64)));
    let AddedTransactionOutcome { hash: pending_hash, .. } =
        node_pool.add_external_transaction(tx).await.unwrap();

    let announcement = match next_message(&mut eth_stream).await {
        EthMessage::NewPooledTransactionHashes72(announcement) => announcement,
        message => panic!("unexpected message {message:?}"),
    };
    assert_eq!(announcement.hashes, vec![pending_hash]);
    // no blob transactions were announced: the zero cell mask on the wire decodes to `None`
    assert_eq!(announcement.cell_mask, None);

    // 2) raw peer -> node: a geth style blob announcement carrying the custody mask
    let pooled = blob_tx_without_blobs();
    let blob_tx_hash = *pooled.tx_hash();
    let sender = pooled.recover_signer().unwrap();
    provider.add_account(sender, ExtendedAccount::new(0, U256::from(10u128.pow(19))));

    let announcement = NewPooledTransactionHashes72 {
        types: vec![EIP4844_TX_TYPE_ID],
        sizes: vec![pooled.encode_2718_len()],
        hashes: vec![blob_tx_hash],
        cell_mask: Some(NewPooledTransactionHashes72::ALL_CELLS_MASK),
    };
    eth_stream.send(EthMessage::NewPooledTransactionHashes72(announcement)).await.unwrap();

    // the node must fetch the announced blob transaction
    let request = match next_message(&mut eth_stream).await {
        EthMessage::GetPooledTransactions(request) => request,
        message => panic!("unexpected message {message:?}"),
    };
    assert_eq!(request.message.0, vec![blob_tx_hash]);

    // 3) raw peer -> node: the eth/72 response elides the blob payloads
    eth_stream
        .send(EthMessage::PooledTransactions(RequestPair {
            request_id: request.request_id,
            message: PooledTransactions(vec![pooled]),
        }))
        .await
        .unwrap();

    // The blob-elided body is dropped before the pool import (cell fetching is not implemented
    // yet), and the eth/72 peer served a spec compliant response so it must not be penalized.
    tokio::time::sleep(Duration::from_secs(1)).await;

    assert!(!node_pool.contains(&blob_tx_hash));
    let reputation = node.peer_handle().peer_by_id(raw_id).await.unwrap().reputation();
    assert_eq!(reputation, baseline_reputation);
    // the session survives serving the blob-elided response
    assert_eq!(node.network().num_connected_peers(), 1);
}
