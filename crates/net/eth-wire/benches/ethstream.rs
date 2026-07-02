#![allow(missing_docs)]

use alloy_consensus::TxLegacy;
use alloy_primitives::{
    bytes::{Bytes, BytesMut},
    Bytes as AlloyBytes, Signature, TxKind, B256, U256,
};
use alloy_rlp::Encodable;
use criterion::{criterion_group, criterion_main, Criterion};
use futures::{future::poll_fn, Sink, SinkExt, Stream, StreamExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    capability::SharedCapabilities, message::EthBroadcastMessage, Capability, EthMessage,
    EthNetworkPrimitives, EthStream, EthVersion, NewPooledTransactionHashes68,
    NewPooledTransactionHashes72, P2PStream, SharedTransactions,
};
use reth_ethereum_primitives::{Transaction, TransactionSigned};
use reth_network_peers::pk2id;
use secp256k1::{SecretKey, SECP256K1};
use std::{
    collections::VecDeque,
    hint::black_box,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

const MESSAGE_COUNT: usize = 1024;
const HASHES_PER_ANNOUNCEMENT: usize = 64;
const ACTIVE_SESSION_P2P_OUTGOING_BUFFER_CAPACITY: usize = 32;
const ECIES_PAYLOAD_LEN: usize = 512;

fn shared_capabilities() -> SharedCapabilities {
    shared_capabilities_for(EthVersion::Eth66)
}

fn shared_capabilities_for(version: EthVersion) -> SharedCapabilities {
    SharedCapabilities::try_new(vec![version.into()], vec![Capability::eth(version)]).unwrap()
}

fn message() -> EthMessage<EthNetworkPrimitives> {
    message_for_version(EthVersion::Eth66)
}

fn message_for_version(version: EthVersion) -> EthMessage<EthNetworkPrimitives> {
    message_for_version_with_hashes(version, 1)
}

fn message_for_version_with_hashes(
    version: EthVersion,
    hash_count: usize,
) -> EthMessage<EthNetworkPrimitives> {
    let hashes = hashes(hash_count);

    if version.is_eth72() {
        return EthMessage::NewPooledTransactionHashes72(NewPooledTransactionHashes72 {
            types: vec![0x02; hash_count],
            sizes: vec![128; hash_count],
            hashes,
            cell_mask: None,
        })
    }

    if version.has_eth68_metadata() {
        return EthMessage::NewPooledTransactionHashes68(NewPooledTransactionHashes68 {
            types: vec![0x02; hash_count],
            sizes: vec![128; hash_count],
            hashes,
        })
    }

    EthMessage::NewPooledTransactionHashes66(hashes.into())
}

fn hashes(count: usize) -> Vec<B256> {
    (0..count).map(|idx| B256::repeat_byte(idx as u8)).collect()
}

fn transaction(nonce: u64) -> Arc<TransactionSigned> {
    Arc::new(TransactionSigned::new_unhashed(
        Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: 1,
            gas_limit: 21_000,
            to: TxKind::Create,
            value: U256::ZERO,
            input: AlloyBytes::from(vec![0x42; 128]),
        }),
        Signature::test_signature(),
    ))
}

fn transaction_broadcast() -> EthBroadcastMessage<EthNetworkPrimitives> {
    EthBroadcastMessage::Transactions(SharedTransactions(vec![transaction(0), transaction(1)]))
}

fn transaction_broadcast_payload_length(
    transactions: &SharedTransactions<TransactionSigned>,
) -> usize {
    transactions.iter().map(Encodable::length).sum()
}

fn eth_stream(transport: MockTransport) -> EthStream<P2PStream<MockTransport>> {
    eth_stream_with_version(transport, EthVersion::Eth66)
}

fn eth_stream_with_version(
    transport: MockTransport,
    version: EthVersion,
) -> EthStream<P2PStream<MockTransport>> {
    EthStream::new(version, P2PStream::new(transport, shared_capabilities_for(version)))
}

fn active_session_eth_stream(transport: MockTransport) -> EthStream<P2PStream<MockTransport>> {
    active_session_eth_stream_with_capacity(transport, ACTIVE_SESSION_P2P_OUTGOING_BUFFER_CAPACITY)
}

fn active_session_eth_stream_with_capacity(
    transport: MockTransport,
    capacity: usize,
) -> EthStream<P2PStream<MockTransport>> {
    let mut stream = eth_stream(transport);
    stream.inner_mut().set_outgoing_message_buffer_capacity(capacity);
    stream
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().unwrap()
}

fn encoded_wire_message() -> BytesMut {
    encoded_wire_message_for_version(EthVersion::Eth66)
}

fn encoded_wire_message_for_version(version: EthVersion) -> BytesMut {
    encoded_wire_message_for_version_with_hashes(version, 1)
}

fn encoded_wire_message_for_version_with_hashes(
    version: EthVersion,
    hash_count: usize,
) -> BytesMut {
    runtime().block_on(async {
        let mut stream = eth_stream_with_version(MockTransport::default(), version);
        stream.send(message_for_version_with_hashes(version, hash_count)).await.unwrap();
        stream.inner().inner().sent[0].clone().into()
    })
}

async fn send_hash_announcements_in_active_session_batches(
    stream: &mut EthStream<P2PStream<MockTransport>>,
    count: usize,
) {
    let mut remaining = count;
    while remaining > 0 {
        poll_fn(|cx| Pin::new(&mut *stream).poll_ready(cx)).await.unwrap();
        let batch = stream.inner().available_outgoing_capacity().min(remaining);
        assert_ne!(batch, 0, "poll_ready returned ready without outgoing capacity");
        for _ in 0..batch {
            Pin::new(&mut *stream).start_send(message()).unwrap();
        }
        remaining -= batch;
    }
    stream.flush().await.unwrap();
}

async fn send_hash_announcements_in_active_session_batches_with_encode_buf(
    stream: &mut EthStream<P2PStream<MockTransport>>,
    count: usize,
) {
    let mut encode_buf = BytesMut::new();
    let mut remaining = count;
    while remaining > 0 {
        poll_fn(|cx| Pin::new(&mut *stream).poll_ready(cx)).await.unwrap();
        let batch = stream.inner().available_outgoing_capacity().min(remaining);
        assert_ne!(batch, 0, "poll_ready returned ready without outgoing capacity");
        for _ in 0..batch {
            stream.start_send_with_encode_buf(message(), &mut encode_buf).unwrap();
        }
        remaining -= batch;
    }
    stream.flush().await.unwrap();
}

async fn send_and_receive_ecies_messages(count: usize) -> usize {
    let server_key = SecretKey::from_slice(&[1; 32]).unwrap();
    let server_id = pk2id(&server_key.public_key(SECP256K1));
    let client_key = SecretKey::from_slice(&[2; 32]).unwrap();
    let (client_io, server_io) = tokio::io::duplex(1024 * 1024);

    let server = tokio::spawn(async move {
        let mut stream = ECIESStream::incoming(server_io, server_key).await.unwrap();
        let mut received = 0;
        while received < count {
            let msg = stream.next().await.unwrap().unwrap();
            black_box(msg.len());
            received += 1;
        }
        received
    });

    let mut client = ECIESStream::connect(client_io, client_key, server_id).await.unwrap();
    let payload = Bytes::from(vec![0x42; ECIES_PAYLOAD_LEN]);
    for _ in 0..count {
        client.send(payload.clone()).await.unwrap();
    }
    client.flush().await.unwrap();

    server.await.unwrap()
}

async fn send_and_receive_ethstream_hash_announcements(count: usize, capacity: usize) -> usize {
    let server_key = SecretKey::from_slice(&[3; 32]).unwrap();
    let server_id = pk2id(&server_key.public_key(SECP256K1));
    let client_key = SecretKey::from_slice(&[4; 32]).unwrap();
    let (client_io, server_io) = tokio::io::duplex(1024 * 1024);

    let server = tokio::spawn(async move {
        let ecies = ECIESStream::incoming(server_io, server_key).await.unwrap();
        let p2p = P2PStream::new(ecies, shared_capabilities());
        let mut stream = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth66, p2p);
        let mut decode_buf = BytesMut::new();
        let mut received = 0;
        while received < count {
            black_box(
                poll_fn(|cx| Pin::new(&mut stream).poll_next_eth_message(cx, &mut decode_buf))
                    .await
                    .unwrap()
                    .unwrap(),
            );
            received += 1;
        }
        received
    });

    let ecies = ECIESStream::connect(client_io, client_key, server_id).await.unwrap();
    let mut p2p = P2PStream::new(ecies, shared_capabilities());
    p2p.set_outgoing_message_buffer_capacity(capacity);
    let mut stream = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth66, p2p);

    let mut remaining = count;
    let mut encode_buf = BytesMut::new();
    while remaining > 0 {
        poll_fn(|cx| Pin::new(&mut stream).poll_ready(cx)).await.unwrap();
        let batch = stream.inner().available_outgoing_capacity().min(remaining);
        assert_ne!(batch, 0, "poll_ready returned ready without outgoing capacity");
        for _ in 0..batch {
            stream.start_send_with_encode_buf(message(), &mut encode_buf).unwrap();
        }
        remaining -= batch;
    }
    poll_fn(|cx| Pin::new(stream.inner_mut()).poll_flush_ecies_buffered(cx)).await.unwrap();

    server.await.unwrap()
}

#[derive(Default)]
struct MockTransport {
    incoming: VecDeque<io::Result<BytesMut>>,
    sent: Vec<Bytes>,
}

impl MockTransport {
    fn with_incoming(message: BytesMut, count: usize) -> Self {
        Self { incoming: (0..count).map(|_| Ok(message.clone())).collect(), sent: Vec::new() }
    }

    fn with_incoming_messages(messages: impl IntoIterator<Item = BytesMut>) -> Self {
        Self { incoming: messages.into_iter().map(Ok).collect(), sent: Vec::new() }
    }
}

impl Stream for MockTransport {
    type Item = io::Result<BytesMut>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.incoming.pop_front())
    }
}

impl Sink<Bytes> for MockTransport {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.sent.push(item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

fn bench_poll_ready_idle(c: &mut Criterion) {
    let mut group = c.benchmark_group("ethstream");
    let rt = runtime();
    group.bench_function("poll_ready_idle", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut stream = eth_stream(MockTransport::default());
                for _ in 0..MESSAGE_COUNT {
                    black_box(poll_fn(|cx| Pin::new(&mut stream).poll_ready(cx)).await).unwrap();
                }
            });
        })
    });
}

fn bench_send_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("ethstream");
    let rt = runtime();
    group.bench_function("send_hash_announcements", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut stream = eth_stream(MockTransport::default());
                for _ in 0..MESSAGE_COUNT {
                    stream.send(message()).await.unwrap();
                }
                black_box(stream.inner().inner().sent.len());
            });
        })
    });

    group.bench_function("send_hash_announcements_batched", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut stream = active_session_eth_stream(MockTransport::default());
                send_hash_announcements_in_active_session_batches(&mut stream, MESSAGE_COUNT).await;
                black_box(stream.inner().inner().sent.len());
            });
        })
    });

    group.bench_function("send_hash_announcements_batched_encode_buf", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut stream = active_session_eth_stream(MockTransport::default());
                send_hash_announcements_in_active_session_batches_with_encode_buf(
                    &mut stream,
                    MESSAGE_COUNT,
                )
                .await;
                black_box(stream.inner().inner().sent.len());
            });
        })
    });

    for version in [EthVersion::Eth68, EthVersion::Eth72] {
        group.bench_function(format!("send_hash_announcements_eth{}", version as u8), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let mut stream = eth_stream_with_version(MockTransport::default(), version);
                    for _ in 0..MESSAGE_COUNT {
                        stream.send(message_for_version(version)).await.unwrap();
                    }
                    black_box(stream.inner().inner().sent.len());
                });
            })
        });

        group.bench_function(
            format!(
                "send_hash_announcements_eth{}_{}_hashes",
                version as u8, HASHES_PER_ANNOUNCEMENT
            ),
            |b| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut stream = eth_stream_with_version(MockTransport::default(), version);
                        for _ in 0..MESSAGE_COUNT {
                            stream
                                .send(message_for_version_with_hashes(
                                    version,
                                    HASHES_PER_ANNOUNCEMENT,
                                ))
                                .await
                                .unwrap();
                        }
                        black_box(stream.inner().inner().sent.len());
                    });
                })
            },
        );
    }

    for capacity in [8, 16, 64] {
        group.bench_function(format!("send_hash_announcements_batched_capacity_{capacity}"), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let mut stream =
                        active_session_eth_stream_with_capacity(MockTransport::default(), capacity);
                    send_hash_announcements_in_active_session_batches(&mut stream, MESSAGE_COUNT)
                        .await;
                    black_box(stream.inner().inner().sent.len());
                });
            })
        });
    }

    group.bench_function("send_transaction_broadcasts", |b| {
        let broadcast = transaction_broadcast();
        b.iter(|| {
            rt.block_on(async {
                let mut stream = eth_stream(MockTransport::default());
                for _ in 0..MESSAGE_COUNT {
                    stream.start_send_broadcast(broadcast.clone()).unwrap();
                    stream.flush().await.unwrap();
                }
                black_box(stream.inner().inner().sent.len());
            });
        })
    });

    group.bench_function("send_transaction_broadcasts_cached_payload_length", |b| {
        let EthBroadcastMessage::Transactions(transactions) = transaction_broadcast() else {
            unreachable!()
        };
        let payload_length = transaction_broadcast_payload_length(&transactions);
        b.iter(|| {
            rt.block_on(async {
                let mut stream = eth_stream(MockTransport::default());
                for _ in 0..MESSAGE_COUNT {
                    stream
                        .start_send_transactions_with_payload_length(
                            transactions.clone(),
                            payload_length,
                        )
                        .unwrap();
                    stream.flush().await.unwrap();
                }
                black_box(stream.inner().inner().sent.len());
            });
        })
    });

    group.bench_function("send_transaction_broadcasts_cached_payload_length_encode_buf", |b| {
        let EthBroadcastMessage::Transactions(transactions) = transaction_broadcast() else {
            unreachable!()
        };
        let payload_length = transaction_broadcast_payload_length(&transactions);
        b.iter(|| {
            rt.block_on(async {
                let mut stream = eth_stream(MockTransport::default());
                let mut encode_buf = BytesMut::new();
                for _ in 0..MESSAGE_COUNT {
                    stream
                        .start_send_transactions_with_payload_length_and_encode_buf(
                            transactions.clone(),
                            payload_length,
                            &mut encode_buf,
                        )
                        .unwrap();
                    stream.flush().await.unwrap();
                }
                black_box(stream.inner().inner().sent.len());
            });
        })
    });
}

fn bench_recv_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("ethstream");
    let rt = runtime();
    group.bench_function("recv_hash_announcements", |b| {
        let encoded = encoded_wire_message();
        b.iter(|| {
            rt.block_on(async {
                let mut stream =
                    eth_stream(MockTransport::with_incoming(encoded.clone(), MESSAGE_COUNT));
                let mut decode_buf = BytesMut::new();
                for _ in 0..MESSAGE_COUNT {
                    black_box(
                        poll_fn(|cx| {
                            Pin::new(&mut stream).poll_next_eth_message(cx, &mut decode_buf)
                        })
                        .await
                        .unwrap()
                        .unwrap(),
                    );
                }
            });
        })
    });

    group.bench_function("recv_hash_announcements_with_ping_controls", |b| {
        let encoded = encoded_wire_message();
        let ping = BytesMut::from(&b"\x02\x01\0\xc0"[..]);
        b.iter(|| {
            rt.block_on(async {
                let incoming = (0..MESSAGE_COUNT).flat_map(|_| [ping.clone(), encoded.clone()]);
                let mut stream = eth_stream(MockTransport::with_incoming_messages(incoming));
                let mut decode_buf = BytesMut::new();
                for _ in 0..MESSAGE_COUNT {
                    black_box(
                        poll_fn(|cx| {
                            Pin::new(&mut stream).poll_next_eth_message(cx, &mut decode_buf)
                        })
                        .await
                        .unwrap()
                        .unwrap(),
                    );
                }
                black_box(stream.inner().inner().sent.len());
            });
        })
    });

    for version in [EthVersion::Eth68, EthVersion::Eth72] {
        group.bench_function(format!("recv_hash_announcements_eth{}", version as u8), |b| {
            let encoded = encoded_wire_message_for_version(version);
            b.iter(|| {
                rt.block_on(async {
                    let mut stream = eth_stream_with_version(
                        MockTransport::with_incoming(encoded.clone(), MESSAGE_COUNT),
                        version,
                    );
                    let mut decode_buf = BytesMut::new();
                    for _ in 0..MESSAGE_COUNT {
                        black_box(
                            poll_fn(|cx| {
                                Pin::new(&mut stream).poll_next_eth_message(cx, &mut decode_buf)
                            })
                            .await
                            .unwrap()
                            .unwrap(),
                        );
                    }
                });
            })
        });

        group.bench_function(
            format!(
                "recv_hash_announcements_eth{}_{}_hashes",
                version as u8, HASHES_PER_ANNOUNCEMENT
            ),
            |b| {
                let encoded =
                    encoded_wire_message_for_version_with_hashes(version, HASHES_PER_ANNOUNCEMENT);
                b.iter(|| {
                    rt.block_on(async {
                        let mut stream = eth_stream_with_version(
                            MockTransport::with_incoming(encoded.clone(), MESSAGE_COUNT),
                            version,
                        );
                        let mut decode_buf = BytesMut::new();
                        for _ in 0..MESSAGE_COUNT {
                            black_box(
                                poll_fn(|cx| {
                                    Pin::new(&mut stream).poll_next_eth_message(cx, &mut decode_buf)
                                })
                                .await
                                .unwrap()
                                .unwrap(),
                            );
                        }
                    });
                })
            },
        );
    }
}

fn bench_ecies_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("eciesstream");
    let rt = runtime();
    group.bench_function("send_receive_messages", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(send_and_receive_ecies_messages(MESSAGE_COUNT).await);
            });
        })
    });
}

fn bench_ethstream_ecies_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("ethstream_ecies");
    let rt = runtime();

    for capacity in [8, 16, 32, 64, 128] {
        group.bench_function(format!("send_hash_announcements_capacity_{capacity}"), |b| {
            b.iter(|| {
                rt.block_on(async {
                    black_box(
                        send_and_receive_ethstream_hash_announcements(MESSAGE_COUNT, capacity)
                            .await,
                    );
                });
            })
        });
    }
}

criterion_group!(
    benches,
    bench_poll_ready_idle,
    bench_send_messages,
    bench_recv_messages,
    bench_ecies_messages,
    bench_ethstream_ecies_messages
);
criterion_main!(benches);
