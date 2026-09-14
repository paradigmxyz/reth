//! Benchmarks for concurrent ExEx WAL access.

#![allow(missing_docs)]

use std::{
    collections::BTreeMap,
    hint::black_box,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use criterion::{criterion_group, criterion_main, Criterion};
use reth_exex::{ExExNotification, Wal};
use reth_provider::Chain;
use reth_testing_utils::generators::{self, random_block};

fn notification() -> ExExNotification {
    let block = random_block(&mut generators::rng(), 1, Default::default())
        .try_recover()
        .expect("generated block should recover");

    ExExNotification::ChainCommitted {
        new: Arc::new(Chain::new(vec![block], Default::default(), BTreeMap::new())),
    }
}

fn lookup_during_commit(c: &mut Criterion) {
    let mut group = c.benchmark_group("exex_wal");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(2));
    group.bench_function("lookup_during_commit", |b| {
        b.iter_custom(|iterations| {
            let directory = tempfile::tempdir().expect("temporary WAL directory should exist");
            let wal = Wal::new(&directory).expect("WAL should initialize");
            let notification = notification();
            let block_hash = notification
                .committed_chain()
                .expect("notification should commit a chain")
                .tip()
                .hash();
            wal.commit(&notification).expect("initial notification should commit");

            let handle = wal.handle();
            let barrier = Arc::new(Barrier::new(2));
            let writer_barrier = barrier.clone();
            let writer = thread::spawn(move || {
                writer_barrier.wait();
                for _ in 0..iterations {
                    wal.commit(&notification).expect("notification should commit");
                }
            });

            barrier.wait();
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(
                    handle
                        .get_committed_notification_by_block_hash(black_box(&block_hash))
                        .expect("notification lookup should succeed"),
                );
            }
            let elapsed = start.elapsed();

            writer.join().expect("WAL writer should not panic");
            elapsed
        });
    });
    group.finish();
}

criterion_group!(benches, lookup_during_commit);
criterion_main!(benches);
