#![allow(missing_docs)]
mod utils;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reth_libmdbx::ObjectLength;
use utils::*;

fn bench_parallel_read(c: &mut Criterion) {
    const NUM_ELEMENTS: u32 = 100_000;
    const NUM_THREADS: usize = 8;

    let (_dir, env) = setup_bench_db(NUM_ELEMENTS);
    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    let dbi = db.dbi();

    let keys: Vec<String> = (0..NUM_ELEMENTS).map(get_key).collect();

    let mut group = c.benchmark_group("parallel_read");

    group.bench_function("shared_txn", |b| {
        b.iter(|| {
            std::thread::scope(|s| {
                for chunk in keys.chunks(keys.len() / NUM_THREADS) {
                    let txn = txn.clone();
                    s.spawn(move || {
                        for key in chunk {
                            black_box(txn.get::<ObjectLength>(dbi, key.as_bytes()).unwrap());
                        }
                    });
                }
            });
        })
    });

    group.bench_function("cloned_txn", |b| {
        b.iter(|| {
            std::thread::scope(|s| {
                for chunk in keys.chunks(keys.len() / NUM_THREADS) {
                    let txn = txn.clone_txn().unwrap();
                    s.spawn(move || {
                        for key in chunk {
                            black_box(txn.get::<ObjectLength>(dbi, key.as_bytes()).unwrap());
                        }
                    });
                }
            });
        })
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = bench_parallel_read
}
criterion_main!(benches);
