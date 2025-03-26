#![allow(missing_docs)]

use alloy_primitives::TxHash;
use criterion::{criterion_group, criterion_main, Criterion};
use pprof::criterion::{Output, PProfProfiler};
use reth_db::{test_utils::create_test_rw_db_with_path, Database, TransactionHashNumbers};
use reth_db_api::transaction::DbTx;
use std::{fs, sync::Arc};

mod utils;
use utils::BENCH_DB_PATH;

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(1, Output::Flamegraph(None)));
    targets = get
}
criterion_main!(benches);

// Small benchmark showing that [get_by_encoded_key] is slightly faster than [get]
// for a reference key, as [get] requires copying or cloning the key first.
fn get(c: &mut Criterion) {
    let mut group = c.benchmark_group("Get");

    // Random keys to get
    let mut keys = Vec::new();
    for _ in 0..10_000_000 {
        let key = TxHash::random();
        keys.push(key);
    }

    // We don't bother mock the DB to reduce noise from DB I/O, value decoding, etc.
    let _ = fs::remove_dir_all(BENCH_DB_PATH);
    let db = Arc::try_unwrap(create_test_rw_db_with_path(BENCH_DB_PATH)).unwrap();
    let tx = db.tx().expect("tx");

    group.bench_function("get", |b| {
        b.iter(|| {
            for key in &keys {
                tx.get::<TransactionHashNumbers>(*key).unwrap();
            }
        })
    });

    group.bench_function("get_by_encoded_key", |b| {
        b.iter(|| {
            for key in &keys {
                tx.get_by_encoded_key::<TransactionHashNumbers>(key).unwrap();
            }
        })
    });
}
