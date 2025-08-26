#![allow(missing_docs)]
use criterion::{criterion_group, criterion_main, Criterion};
use reth_libmdbx::{Environment, ObjectLength, WriteFlags};
use std::{
    sync::{Arc, Barrier},
    thread,
};

fn setup_test_env() -> (tempfile::TempDir, Environment) {
    let dir = tempfile::tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();
    {
        let rw_txn = env.begin_rw_txn().unwrap();
        let db = rw_txn.create_db(None, Default::default()).unwrap();

        for i in 0..1000u32 {
            let key = format!("key{i:06}");
            let value = i.to_le_bytes();
            rw_txn.put(db.dbi(), &key, value, WriteFlags::empty()).unwrap();
        }

        rw_txn.commit().unwrap();
    }

    (dir, env)
}

fn bench_sequential_reads(c: &mut Criterion) {
    let (_dir, env) = setup_test_env();
    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(None).unwrap();

    c.bench_function("sequential_reads", |b| {
        b.iter(|| {
            let mut sum = 0u32;
            for i in 0..100u32 {
                let key = format!("key{i:06}");
                if let Some(value) = txn.get::<ObjectLength>(db.dbi(), key.as_bytes()).unwrap() {
                    sum += *value as u32;
                }
            }
            sum
        });
    });
}

fn bench_concurrent_reads(c: &mut Criterion) {
    let (_dir, env) = setup_test_env();

    c.bench_function("concurrent_reads", |b| {
        b.iter(|| {
            let num_threads = 4;
            let barrier = Arc::new(Barrier::new(num_threads));
            let mut handles = vec![];

            for thread_id in 0..num_threads {
                let env = env.clone();
                let barrier = Arc::clone(&barrier);

                let handle = thread::spawn(move || {
                    barrier.wait();

                    let txn = env.begin_ro_txn().unwrap();
                    let db = txn.open_db(None).unwrap();

                    let mut sum = 0u32;
                    for i in 0..25u32 {
                        let key = format!("key{:06}", (i + thread_id as u32 * 25) % 1000);
                        if let Some(value) =
                            txn.get::<ObjectLength>(db.dbi(), key.as_bytes()).unwrap()
                        {
                            sum += *value as u32;
                        }
                    }
                    sum
                });

                handles.push(handle);
            }

            let mut total = 0u32;
            for handle in handles {
                total += handle.join().unwrap();
            }
            total
        });
    });
}

fn bench_mixed_workload(c: &mut Criterion) {
    let (_dir, env) = setup_test_env();

    c.bench_function("mixed_workload", |b| {
        b.iter(|| {
            let num_readers = 3;
            let barrier = Arc::new(Barrier::new(num_readers + 1));
            let mut handles = vec![];

            // Spawn reader threads
            for thread_id in 0..num_readers {
                let env = env.clone();
                let barrier = Arc::clone(&barrier);

                let handle = thread::spawn(move || {
                    barrier.wait();

                    let txn = env.begin_ro_txn().unwrap();
                    let db = txn.open_db(None).unwrap();

                    let mut sum = 0u32;
                    for i in 0..20u32 {
                        let key = format!("key{:06}", (i + thread_id as u32 * 20) % 1000);
                        if let Some(value) =
                            txn.get::<ObjectLength>(db.dbi(), key.as_bytes()).unwrap()
                        {
                            sum += *value as u32;
                        }
                    }
                    sum
                });

                handles.push(handle);
            }

            // Writer thread
            let env_writer = env.clone();
            let barrier_writer = Arc::clone(&barrier);
            let writer_handle = thread::spawn(move || {
                barrier_writer.wait();

                let rw_txn = env_writer.begin_rw_txn().unwrap();
                let db = rw_txn.open_db(None).unwrap();

                // Do a few writes while readers are running
                for i in 1000..1005u32 {
                    let key = format!("key{i:06}");
                    let value = i.to_le_bytes();
                    rw_txn.put(db.dbi(), &key, value, WriteFlags::empty()).unwrap();
                }

                rw_txn.commit().unwrap();
                42u32
            });

            let mut total = 0u32;
            for handle in handles {
                total += handle.join().unwrap();
            }
            total += writer_handle.join().unwrap();
            total
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = bench_sequential_reads, bench_concurrent_reads, bench_mixed_workload
}
criterion_main!(benches);
