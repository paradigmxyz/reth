#![allow(missing_docs)]

//! Benchmarks for the full outbound/inbound message pipeline:
//! `EthStream` (RLP) -> `P2PStream` (snappy + message ids) -> `ECIESStream` (framing, AES, MAC)
//! over an in-memory duplex transport.
//!
//! This exercises the same stack an [`ActiveSession`] drives for transaction propagation,
//! excluding the socket syscalls.

use alloy_chains::NamedChain;
use alloy_consensus::{EthereumTxEnvelope, SignableTransaction, TxEip1559, TxEip4844};
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::{SinkExt, StreamExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    message::EthBroadcastMessage, EthMessage, EthNetworkPrimitives, EthStream,
    HelloMessageWithProtocols, P2PStream, ProtocolVersion, Status, StatusMessage,
    UnauthedEthStream, UnauthedP2PStream, UnifiedStatus,
};
use reth_eth_wire_types::{
    broadcast::{NewPooledTransactionHashes68, SharedTransactions},
    EthVersion,
};
use reth_ethereum_forks::{ForkFilter, Head};
use reth_network_peers::pk2id;
use secp256k1::{SecretKey, SECP256K1};
use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{io::DuplexStream, runtime::Runtime};

type BenchTx = EthereumTxEnvelope<TxEip4844>;
type BenchStream = EthStream<P2PStream<ECIESStream<DuplexStream>>, EthNetworkPrimitives>;

/// Number of messages fed into the sink before driving a flush, mimicking how the active session
/// flushes the connection once per queued batch.
const FLUSH_BATCH: usize = 16;

fn hello(key: &SecretKey) -> HelloMessageWithProtocols {
    HelloMessageWithProtocols {
        protocol_version: ProtocolVersion::V5,
        client_version: "bench/1.0.0".to_string(),
        protocols: vec![EthVersion::Eth68.into()],
        port: 30303,
        id: pk2id(&key.public_key(SECP256K1)),
    }
}

/// Sets up a connected `EthStream<P2PStream<ECIESStream<DuplexStream>>>` pair via the regular
/// handshakes over an in-memory duplex pipe.
async fn connected_pair() -> (BenchStream, BenchStream) {
    let (client_io, server_io) = tokio::io::duplex(1 << 22);

    let server_key = SecretKey::new(&mut rand_08::thread_rng());
    let client_key = SecretKey::new(&mut rand_08::thread_rng());
    let server_id = pk2id(&server_key.public_key(SECP256K1));

    let genesis = B256::repeat_byte(0x11);
    let fork_filter = ForkFilter::new(Head::default(), genesis, 0, Vec::new());
    let status = UnifiedStatus::from_message(StatusMessage::Legacy(Status {
        version: EthVersion::Eth68,
        chain: NamedChain::Mainnet.into(),
        total_difficulty: U256::ZERO,
        blockhash: B256::repeat_byte(0x22),
        genesis,
        forkid: fork_filter.current(),
    }));

    let server_fork_filter = fork_filter.clone();
    let server = async move {
        let ecies = ECIESStream::incoming(server_io, server_key).await.unwrap();
        let (p2p, _) = UnauthedP2PStream::new(ecies).handshake(hello(&server_key)).await.unwrap();
        let (eth, _) = UnauthedEthStream::new(p2p)
            .handshake::<EthNetworkPrimitives>(status, server_fork_filter)
            .await
            .unwrap();
        eth
    };
    let client = async move {
        let ecies = ECIESStream::connect(client_io, client_key, server_id).await.unwrap();
        let (p2p, _) = UnauthedP2PStream::new(ecies).handshake(hello(&client_key)).await.unwrap();
        let (eth, _) = UnauthedEthStream::new(p2p)
            .handshake::<EthNetworkPrimitives>(status, fork_filter)
            .await
            .unwrap();
        eth
    };

    tokio::join!(client, server)
}

/// Creates `n` signed EIP-1559 transactions with an input payload of `payload` bytes.
fn make_txs(n: usize, payload: usize) -> Vec<Arc<BenchTx>> {
    (0..n)
        .map(|i| {
            let tx = TxEip1559 {
                chain_id: 1,
                nonce: i as u64,
                gas_limit: 21_000,
                max_fee_per_gas: 1_000_000_000,
                max_priority_fee_per_gas: 1_000_000,
                to: TxKind::Call(Address::repeat_byte(0x42)),
                value: U256::from(1u64),
                input: Bytes::from(vec![0xa5; payload]),
                access_list: Default::default(),
            };
            let sig = Signature::new(U256::from(1u64), U256::from(2u64), false);
            Arc::new(EthereumTxEnvelope::Eip1559(tx.into_signed(sig)))
        })
        .collect()
}

/// Runs the send loop for `iters` messages on a fresh connection, returning the elapsed time from
/// first send until the receiver has decoded all messages.
async fn run_pipeline<M>(iters: u64, make_msg: M) -> Duration
where
    M: Fn() -> OutboundMessage,
{
    let (mut client, mut server) = connected_pair().await;

    let receiver = tokio::spawn(async move {
        let mut received = 0u64;
        while received < iters {
            match server.next().await {
                Some(Ok(msg)) => {
                    black_box(&msg);
                    received += 1;
                }
                other => panic!("receiver failed: {:?}", other.map(|r| r.map(|_| ()))),
            }
        }
        server
    });

    let start = Instant::now();
    let mut in_batch = 0usize;
    for _ in 0..iters {
        match make_msg() {
            OutboundMessage::Broadcast(msg) => {
                futures::future::poll_fn(|cx| client.poll_ready_unpin(cx)).await.unwrap();
                client.start_send_broadcast(msg).unwrap();
            }
            OutboundMessage::Message(msg) => {
                client.feed(msg).await.unwrap();
            }
        }
        in_batch += 1;
        if in_batch == FLUSH_BATCH {
            client.flush().await.unwrap();
            in_batch = 0;
        }
    }
    client.flush().await.unwrap();

    let server = receiver.await.unwrap();
    let elapsed = start.elapsed();
    drop(server);
    drop(client);
    elapsed
}

enum OutboundMessage {
    Broadcast(EthBroadcastMessage<EthNetworkPrimitives>),
    Message(EthMessage<EthNetworkPrimitives>),
}

fn bench_pipeline(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("eth_wire_pipeline");
    group.sample_size(10);

    // full transactions broadcasts of varying batch sizes
    for &num_txs in &[1usize, 25, 100] {
        let txs = make_txs(num_txs, 100);
        let msg_len =
            alloy_rlp::encode(SharedTransactions::<BenchTx>(txs.clone())).len() as u64 + 1;
        group.throughput(Throughput::Bytes(msg_len));
        group.bench_with_input(BenchmarkId::new("txs_broadcast", num_txs), &num_txs, |b, _| {
            b.iter_custom(|iters| {
                rt.block_on(run_pipeline(iters, || {
                    OutboundMessage::Broadcast(EthBroadcastMessage::Transactions(
                        SharedTransactions(txs.clone()),
                    ))
                }))
            });
        });
    }

    // hash announcements
    let num_hashes = 256usize;
    let hashes = NewPooledTransactionHashes68 {
        types: vec![2u8; num_hashes],
        sizes: vec![250; num_hashes],
        hashes: (0..num_hashes).map(|i| B256::with_last_byte(i as u8)).collect(),
    };
    let msg_len = alloy_rlp::encode(&hashes).len() as u64 + 1;
    group.throughput(Throughput::Bytes(msg_len));
    group.bench_with_input(BenchmarkId::new("hashes68", num_hashes), &num_hashes, |b, _| {
        b.iter_custom(|iters| {
            rt.block_on(run_pipeline(iters, || {
                OutboundMessage::Message(EthMessage::NewPooledTransactionHashes68(hashes.clone()))
            }))
        });
    });

    group.finish();
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);
