#![allow(missing_docs)]

use alloy_consensus::TxLegacy;
use alloy_primitives::{
    bytes::{Bytes, BytesMut},
    Bytes as AlloyBytes, Signature, TxKind, B256, U256,
};
use criterion::{criterion_group, criterion_main, Criterion};
use futures::{future::poll_fn, Sink, SinkExt, Stream};
use reth_eth_wire::{
    capability::SharedCapabilities, message::EthBroadcastMessage, Capability, EthMessage,
    EthNetworkPrimitives, EthStream, EthVersion, P2PStream, SharedTransactions,
};
use reth_ethereum_primitives::{Transaction, TransactionSigned};
use std::{
    collections::VecDeque,
    hint::black_box,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

const MESSAGE_COUNT: usize = 1024;
const ACTIVE_SESSION_P2P_OUTGOING_BUFFER_CAPACITY: usize = 32;

fn shared_capabilities() -> SharedCapabilities {
    SharedCapabilities::try_new(
        vec![EthVersion::Eth66.into()],
        vec![Capability::eth(EthVersion::Eth66)],
    )
    .unwrap()
}

fn message() -> EthMessage<EthNetworkPrimitives> {
    EthMessage::NewPooledTransactionHashes66(vec![B256::repeat_byte(0x42)].into())
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

fn eth_stream(transport: MockTransport) -> EthStream<P2PStream<MockTransport>> {
    EthStream::new(EthVersion::Eth66, P2PStream::new(transport, shared_capabilities()))
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
    tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap()
}

fn encoded_wire_message() -> BytesMut {
    runtime().block_on(async {
        let mut stream = eth_stream(MockTransport::default());
        stream.send(message()).await.unwrap();
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

#[derive(Default)]
struct MockTransport {
    incoming: VecDeque<io::Result<BytesMut>>,
    sent: Vec<Bytes>,
}

impl MockTransport {
    fn with_incoming(message: BytesMut, count: usize) -> Self {
        Self { incoming: (0..count).map(|_| Ok(message.clone())).collect(), sent: Vec::new() }
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
}

criterion_group!(benches, bench_poll_ready_idle, bench_send_messages, bench_recv_messages);
criterion_main!(benches);
