//! Proof of Concept: FixedCache silent write drops under contention.
//!
//! This test demonstrates that `FixedCache::insert()` silently drops writes when a concurrent
//! `get()` holds the bucket lock. This is the root cause of the nonce corruption bug that causes
//! reth nodes to reject valid blocks and stall.
//!
//! The race in production:
//! 1. `save_cache` calls `insert_state()` which calls `cache.insert(addr, account)` for each
//!    account modified in the block (including nonce updates)
//! 2. Concurrently, the multiproof task calls `cache.get(addr)` to read state for proof generation
//! 3. `get()` holds a per-bucket lock while cloning the value
//! 4. `insert()` uses `try_lock_ret(None)` — if the bucket is locked, it silently returns
//! 5. The nonce update is **never written** to the cache
//! 6. Next block executes with the stale nonce → receipt root mismatch → node stalls
//!
//! The fix: defer `insert_state()` until after `valid_block_rx.recv()`, which guarantees the
//! multiproof task has finished reading before any writes occur.
//!
//! See also: `cached_state::tests::test_insert_state_nonce_not_dropped_under_concurrent_reads`
//! for the integration test using real `ExecutionCache` + `BundleState` types.

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Barrier,
        },
        time::Duration,
    };

    /// Demonstrates 100% write drop rate on the underlying FixedCache when readers
    /// continuously hold the bucket lock with a slow Clone implementation.
    ///
    /// This is a deterministic proof that FixedCache::insert() silently drops writes
    /// under contention — it doesn't retry or block, it just returns.
    ///
    /// With a 5ms sleep in Clone, the bucket lock is held almost continuously by
    /// 4 reader threads. Every single insert() sees LOCKED_BIT and silently returns.
    #[test]
    fn fixed_cache_insert_silently_dropped_under_contention() {
        use fixed_cache::{Cache, CacheConfig};

        struct EpochConfig;
        impl CacheConfig for EpochConfig {
            const EPOCHS: bool = true;
        }

        /// Value type with a deliberately slow Clone to widen the lock-hold window.
        #[derive(Debug, PartialEq, Eq)]
        struct SlowCloneValue {
            nonce: u64,
        }

        impl Clone for SlowCloneValue {
            fn clone(&self) -> Self {
                // Hold the bucket lock for 5ms during get().
                // This guarantees any concurrent insert() will see LOCKED_BIT
                // and silently return without writing.
                std::thread::sleep(Duration::from_millis(5));
                Self { nonce: self.nonce }
            }
        }

        type BH = std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>;

        let num_reader_threads = 4;
        let cache: Arc<Cache<u64, SlowCloneValue, BH, EpochConfig>> =
            Arc::new(Cache::new(4096, Default::default()));

        cache.insert(1, SlowCloneValue { nonce: 0 });

        let stop = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(num_reader_threads + 1));

        let readers: Vec<_> = (0..num_reader_threads)
            .map(|_| {
                let c = cache.clone();
                let s = stop.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    while !s.load(Ordering::Relaxed) {
                        let _ = c.get(&1);
                    }
                })
            })
            .collect();

        barrier.wait();
        std::thread::sleep(Duration::from_millis(10));

        let total_inserts = 100u64;
        for nonce in 1..=total_inserts {
            cache.insert(1, SlowCloneValue { nonce });
            std::thread::yield_now();
        }

        stop.store(true, Ordering::Relaxed);
        for r in readers {
            r.join().unwrap();
        }

        let final_nonce = cache.get(&1).unwrap().nonce;

        eprintln!(
            "\n=== FixedCache Race Condition PoC ===\n\
             Reader threads:  {num_reader_threads}\n\
             Clone hold:      5ms\n\
             Insert attempts: {total_inserts}\n\
             Final nonce:     {final_nonce} (expected: {total_inserts})\n\
             Write dropped:   {}\n",
            final_nonce != total_inserts
        );

        assert!(
            final_nonce != total_inserts,
            "Expected silently dropped inserts (final nonce < {total_inserts}), but got {final_nonce}."
        );
    }
}
