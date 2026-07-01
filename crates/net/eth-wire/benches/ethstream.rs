#![allow(missing_docs)]

use alloy_primitives::{
    bytes::{Bytes, BytesMut},
    B256,
};
use criterion::{criterion_group, criterion_main, Criterion};
use futures::{future::poll_fn, Sink, SinkExt, Stream, StreamExt};
use reth_eth_wire::{
    capability::SharedCapabilities, Capability, EthMessage, EthNetworkPrimitives, EthStream,
    EthVersion, P2PStream,
};
use std::{
    collections::VecDeque,
    hint::black_box,
    io,
    pin::Pin,
    task::{Context, Poll},
};

const MESSAGE_COUNT: usize = 1024;

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

fn eth_stream(transport: MockTransport) -> EthStream<P2PStream<MockTransport>> {
    EthStream::new(EthVersion::Eth66, P2PStream::new(transport, shared_capabilities()))
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
                for _ in 0..MESSAGE_COUNT {
                    black_box(stream.next().await.unwrap().unwrap());
                }
            });
        })
    });
}

criterion_group!(benches, bench_poll_ready_idle, bench_send_messages, bench_recv_messages);
criterion_main!(benches);
