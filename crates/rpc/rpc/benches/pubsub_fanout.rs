#![allow(missing_docs)]

//! Per-subscriber cost of eth pubsub notifications.
//!
//! Serializing every notification for every subscriber means an item that reaches N subscribers is
//! serialized N times. Shared feeds serialize each item once into a `SharedResult`, and sending it
//! to a subscriber only splices it into that subscriber's notification. Both paths go through a
//! real `SubscriptionSink` and are checked to produce identical notifications.
//!
//! Needs real data: set `PUBSUB_BENCH_DATA` to a directory holding `header.json` and `txs.json`
//! (an RPC header and the full transactions of one block), and optionally `receipts.json` (the
//! receipts of one block).

use alloy_rpc_types_eth::{Header, Transaction, TransactionReceipt};
use criterion::{criterion_group, criterion_main, Criterion};
use jsonrpsee::{
    server::{SharedResult, SubscriptionMessage},
    PendingSubscriptionSink, RpcModule, SubscriptionSink,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{hint::black_box, path::Path};
use tokio::sync::mpsc;

/// Notification method of eth pubsub.
const METHOD: &str = "eth_subscription";

/// Capacity of the subscriber's connection channel.
const CHANNEL_CAPACITY: usize = 16;

/// A subscription and the connection channel its notifications go to.
struct Subscriber {
    sink: SubscriptionSink,
    notifications: mpsc::Receiver<String>,
}

impl Subscriber {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            let (pending_tx, mut pending_rx) = mpsc::unbounded_channel::<PendingSubscriptionSink>();
            let mut module = RpcModule::new(pending_tx);
            module
                .register_subscription_raw("subscribe", METHOD, "unsubscribe", |_, pending, ctx| {
                    ctx.send(pending).unwrap();
                })
                .unwrap();
            let request = r#"{"jsonrpc":"2.0","id":1,"method":"subscribe"}"#;
            let (response, sink) =
                tokio::join!(module.raw_json_request(request, CHANNEL_CAPACITY), async {
                    pending_rx.recv().await.unwrap().accept().await.unwrap()
                });
            let (_, notifications) = response.unwrap();
            Self { sink, notifications }
        })
    }

    /// Sends `msg` and returns the notification the connection receives.
    fn send(&mut self, msg: SubscriptionMessage) -> String {
        self.sink.try_send(msg).unwrap();
        self.notifications.try_recv().unwrap()
    }

    /// Sends `result` serialized for this subscriber alone.
    fn send_serialized(&mut self, result: &impl Serialize) -> String {
        let msg =
            SubscriptionMessage::new(self.sink.method_name(), self.sink.subscription_id(), result)
                .unwrap();
        self.send(msg)
    }

    /// Sends a result that was serialized once for all subscribers.
    fn send_shared(&mut self, result: &SharedResult) -> String {
        self.send(SubscriptionMessage::from_shared(result.clone()))
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> T {
    serde_json::from_slice(&std::fs::read(path).unwrap())
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// Serializes `items` once each, checking that sending them shared is byte for byte the same as
/// sending them serialized.
fn shared_results<T: Serialize>(subscriber: &mut Subscriber, items: &[T]) -> Vec<SharedResult> {
    items
        .iter()
        .map(|item| {
            let shared = SharedResult::new(item).unwrap();
            assert_eq!(subscriber.send_shared(&shared), subscriber.send_serialized(item));
            shared
        })
        .collect()
}

/// Benchmarks sending `items` to one subscriber: serialized for it, as a shared result, and the
/// one-time serialization of the shared results.
fn bench_items<T: Serialize>(c: &mut Criterion, group: &str, items: &[T]) {
    let mut subscriber = Subscriber::new();
    let shared = shared_results(&mut subscriber, items);

    let mut g = c.benchmark_group(group);
    g.bench_function("serialize per subscriber", |b| {
        b.iter(|| {
            for item in items {
                black_box(subscriber.send_serialized(black_box(item)));
            }
        })
    });
    g.bench_function("shared: splice per subscriber", |b| {
        b.iter(|| {
            for result in &shared {
                black_box(subscriber.send_shared(black_box(result)));
            }
        })
    });
    g.bench_function("shared: serialize once (amortized over N)", |b| {
        b.iter(|| {
            for item in items {
                black_box(SharedResult::new(black_box(item)).unwrap());
            }
        })
    });
    g.finish();
}

fn bench(c: &mut Criterion) {
    let dir = std::env::var_os("PUBSUB_BENCH_DATA")
        .expect("set PUBSUB_BENCH_DATA to a dir with header.json and txs.json");
    let dir = Path::new(&dir);
    let header: Header = read(&dir.join("header.json"));
    let txs: Vec<Transaction> = read(&dir.join("txs.json"));
    let hashes: Vec<_> = txs.iter().map(|tx| *tx.inner.tx_hash()).collect();

    bench_items(c, "newHeads per block", &[header]);
    bench_items(c, "full pending txs per block of txs", &txs);
    bench_items(c, "pending tx hashes per block of txs", &hashes);

    // An unfiltered `transactionReceipts` subscriber gets every receipt of a block in one
    // notification, the largest payload to fan out. Optional, because receipts are pruned quickly
    // on the bench node.
    if dir.join("receipts.json").exists() {
        let receipts: Vec<TransactionReceipt> = read(&dir.join("receipts.json"));
        bench_items(c, "unfiltered receipts per block", &[receipts]);
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
