#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reth_engine_tree::tree::{CacheWaitBenchHarness, WaitForCaches};
use reth_tasks::Runtime;
use std::{
    hint::spin_loop,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const HOLD_DURATION: Duration = Duration::from_micros(400);
const MAX_UNCONTENDED_WAIT: Duration = Duration::from_millis(1);

struct LockBlocker {
    start_tx: Sender<()>,
    handle: JoinHandle<()>,
}

impl LockBlocker {
    fn start(self) -> JoinHandle<()> {
        self.start_tx.send(()).expect("blocker start signal should be delivered");
        self.handle
    }
}

fn spin_for(duration: Duration) {
    let start = Instant::now();
    while start.elapsed() < duration {
        spin_loop();
    }
}

fn assert_contended(wait: Duration, elapsed: Duration) {
    assert!(wait > Duration::ZERO);
    assert!(wait <= elapsed);
}

fn spawn_execution_cache_blocker(harness: CacheWaitBenchHarness) -> LockBlocker {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (start_tx, start_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        harness.with_execution_cache_blocked(|| {
            ready_tx.send(()).expect("execution cache blocker should report readiness");
            start_rx.recv().expect("execution cache blocker should receive the start signal");
            spin_for(HOLD_DURATION);
        });
    });
    ready_rx.recv().expect("execution cache blocker should acquire the lock");
    LockBlocker { start_tx, handle }
}

fn spawn_sparse_trie_blocker(harness: CacheWaitBenchHarness) -> LockBlocker {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (start_tx, start_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        harness.with_sparse_trie_blocked(|| {
            ready_tx.send(()).expect("sparse trie blocker should report readiness");
            start_rx.recv().expect("sparse trie blocker should receive the start signal");
            spin_for(HOLD_DURATION);
        });
    });
    ready_rx.recv().expect("sparse trie blocker should acquire the lock");
    LockBlocker { start_tx, handle }
}

fn bench_wait_for_caches(c: &mut Criterion) {
    let mut group = c.benchmark_group("wait_for_caches");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(2));

    let uncontended = CacheWaitBenchHarness::new(Runtime::test());
    group.bench_function("uncontended", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let wait = uncontended.wait_for_caches();
                total += start.elapsed();
                assert!(wait.execution_cache <= MAX_UNCONTENDED_WAIT);
                assert!(wait.sparse_trie <= MAX_UNCONTENDED_WAIT);
                black_box(wait);
            }
            total
        });
    });

    let sparse_trie_contended = CacheWaitBenchHarness::new(Runtime::test());
    group.bench_function("sparse_trie_contended", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let blocker = spawn_sparse_trie_blocker(sparse_trie_contended.clone());
                let start = Instant::now();
                let handle = blocker.start();
                let wait = sparse_trie_contended.wait_for_caches();
                let elapsed = start.elapsed();
                total += elapsed;

                assert!(wait.execution_cache <= MAX_UNCONTENDED_WAIT);
                assert_contended(wait.sparse_trie, elapsed);
                black_box(wait);

                handle.join().expect("sparse trie blocker thread should complete");
            }
            total
        });
    });

    let dual_contended = CacheWaitBenchHarness::new(Runtime::test());
    group.bench_function("dual_contended", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let execution_blocker = spawn_execution_cache_blocker(dual_contended.clone());
                let sparse_trie_blocker = spawn_sparse_trie_blocker(dual_contended.clone());

                let start = Instant::now();
                let execution_handle = execution_blocker.start();
                let sparse_trie_handle = sparse_trie_blocker.start();
                let wait = dual_contended.wait_for_caches();
                let elapsed = start.elapsed();
                total += elapsed;

                assert_contended(wait.execution_cache, elapsed);
                assert_contended(wait.sparse_trie, elapsed);
                black_box(wait);

                execution_handle.join().expect("execution cache blocker thread should complete");
                sparse_trie_handle.join().expect("sparse trie blocker thread should complete");
            }
            total
        });
    });

    group.finish();
}

criterion_group!(benches, bench_wait_for_caches);
criterion_main!(benches);
