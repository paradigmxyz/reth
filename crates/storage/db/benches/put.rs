#![allow(missing_docs)]

use alloy_primitives::B256;
use criterion::{criterion_group, criterion_main, Criterion};
use reth_db::{test_utils::create_test_rw_db_with_path, CanonicalHeaders, Database};
use reth_db_api::transaction::DbTxMut;

mod utils;
use utils::BENCH_DB_PATH;

const NUM_BLOCKS: u64 = 1_000_000;

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = put
}
criterion_main!(benches);

// Small benchmark showing that `append` is much faster than `put` when keys are put in order
fn put(c: &mut Criterion) {
    let mut group = c.benchmark_group("Put");

    let setup = || {
        let _ = std::fs::remove_dir_all(BENCH_DB_PATH);
        create_test_rw_db_with_path(BENCH_DB_PATH).tx_mut().expect("tx")
    };

    group.bench_function("put", |b| {
        b.iter_with_setup(setup, |tx| {
            for i in 0..NUM_BLOCKS {
                tx.put::<CanonicalHeaders>(i, B256::ZERO).unwrap();
            }
        })
    });

    group.bench_function("append", |b| {
        b.iter_with_setup(setup, |tx| {
            for i in 0..NUM_BLOCKS {
                tx.append::<CanonicalHeaders>(i, B256::ZERO).unwrap();
            }
        })
    });
}
