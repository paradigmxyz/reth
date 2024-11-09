#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};


use alloy_primitives::address;
use reth_transaction_pool::test_utils::{MockTransaction, MockTransactionFactory};
use std::collections::{BTreeMap, HashMap};

fn bench_map_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("map_operations");
    let mut f = MockTransactionFactory::default();

    // Setup test data
    let sender = address!("000000000000000000000000000000000000000a");
    let transactions: Vec<_> = (0..1000)
        .map(|i| f.validated_arc(MockTransaction::eip1559().with_sender(sender).with_nonce(i)))
        .collect();

    // Setup maps with same data
    let btree_data: BTreeMap<_, _> =
        transactions.iter().map(|tx| (tx.transaction_id, tx.clone())).collect();
    let hash_data: HashMap<_, _> =
        transactions.iter().map(|tx| (tx.transaction_id, tx.clone())).collect();

    // Benchmark lookups
    group.bench_function("btreemap_lookup", |b| {
        let id = transactions[500].transaction_id;
        b.iter(|| {
            black_box(btree_data.get(black_box(&id)));
        });
    });

    group.bench_function("hashmap_lookup", |b| {
        let id = transactions[500].transaction_id;
        b.iter(|| {
            black_box(hash_data.get(black_box(&id)));
        });
    });

    // Benchmark insertions
    group.bench_function("btreemap_insert", |b| {
        b.iter(|| {
            let mut map = BTreeMap::new();
            for tx in transactions.iter().take(100) {
                black_box(map.insert(tx.transaction_id, tx.clone()));
            }
        });
    });

    group.bench_function("hashmap_insert", |b| {
        b.iter(|| {
            let mut map = HashMap::with_capacity(100);
            for tx in transactions.iter().take(100) {
                black_box(map.insert(tx.transaction_id, tx.clone()));
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_map_operations);
criterion_main!(benches);
