#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reth_engine_tree::tree::{wait_for_caches_bench, CacheAvailabilityWait};
use reth_tasks::Runtime;
use std::{
    hint::spin_loop,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

// Keep the synthetic contention short so this benchmark measures the wrapper overhead around the
// reported wait duration, which matches the near-zero waits observed in production traces.
const HOLD_DURATION: Duration = Duration::from_micros(1);

#[derive(Clone, Debug, Default)]
struct MockAvailability {
    blocked_for_nanos: Arc<AtomicU64>,
}

impl MockAvailability {
    fn block_for(&self, duration: Duration) {
        let nanos: u64 = duration
            .as_nanos()
            .try_into()
            .expect("benchmark hold duration should fit in u64 nanoseconds");
        self.blocked_for_nanos.store(nanos, Ordering::Release);
    }
}

impl CacheAvailabilityWait for MockAvailability {
    fn try_wait_for_availability(&self) -> Option<Duration> {
        (self.blocked_for_nanos.load(Ordering::Acquire) == 0).then_some(Duration::ZERO)
    }

    fn wait_for_availability(&self) -> Duration {
        let duration = Duration::from_nanos(self.blocked_for_nanos.swap(0, Ordering::AcqRel));
        if !duration.is_zero() {
            spin_for(duration);
        }
        duration
    }
}

fn spin_for(duration: Duration) {
    let start = Instant::now();
    while start.elapsed() < duration {
        spin_loop();
    }
}

fn bench_wait_for_caches(c: &mut Criterion) {
    let mut group = c.benchmark_group("wait_for_caches");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(2));

    let runtime = Runtime::test();
    let execution_cache = MockAvailability::default();
    let sparse_trie = MockAvailability::default();
    group.bench_function("uncontended", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let wait = wait_for_caches_bench(&runtime, &execution_cache, &sparse_trie);
                total += start.elapsed();
                assert_eq!(wait.execution_cache, Duration::ZERO);
                assert_eq!(wait.sparse_trie, Duration::ZERO);
                black_box(wait);
            }
            total
        });
    });

    let runtime = Runtime::test();
    let execution_cache = MockAvailability::default();
    let sparse_trie = MockAvailability::default();
    group.bench_function("sparse_trie_contended", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                sparse_trie.block_for(HOLD_DURATION);

                let start = Instant::now();
                let wait = wait_for_caches_bench(&runtime, &execution_cache, &sparse_trie);
                total += start.elapsed();

                assert_eq!(wait.execution_cache, Duration::ZERO);
                assert_eq!(wait.sparse_trie, HOLD_DURATION);
                black_box(wait);
            }
            total
        });
    });

    let runtime = Runtime::test();
    let execution_cache = MockAvailability::default();
    let sparse_trie = MockAvailability::default();
    group.bench_function("dual_contended", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                execution_cache.block_for(HOLD_DURATION);
                sparse_trie.block_for(HOLD_DURATION);

                let start = Instant::now();
                let wait = wait_for_caches_bench(&runtime, &execution_cache, &sparse_trie);
                total += start.elapsed();

                assert_eq!(wait.execution_cache, HOLD_DURATION);
                assert_eq!(wait.sparse_trie, HOLD_DURATION);
                black_box(wait);
            }
            total
        });
    });

    group.finish();
}

criterion_group!(benches, bench_wait_for_caches);
criterion_main!(benches);
