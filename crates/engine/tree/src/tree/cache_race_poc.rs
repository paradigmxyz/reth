//! Proof of Concept: FixedCache silent write drops under contention.
//!
//! This test demonstrates that `FixedCache::insert()` silently drops writes when a concurrent
//! `get()` holds the bucket lock. This is the root cause of the nonce corruption bug that causes
//! reth nodes to reject valid blocks and stall on bal-devnet-2.
//!
//! The race in production:
//! 1. `save_cache` calls `insert_state()` which calls `cache.insert(addr, account)` for each
//!    account modified in the block (including nonce updates)
//! 2. Concurrently, the multiproof task calls `cache.get(addr)` to read state for proof generation
//! 3. `get()` holds a per-bucket lock while cloning the value
//! 4. `insert()` uses `try_lock_ret(None)` — if the bucket is locked, it silently returns
//! 5. The nonce update is **never written** to the cache
//! 6. Next block executes with the stale nonce → receipt root mismatch → node stalls

#[cfg(test)]
mod tests {
    use fixed_cache::{Cache, CacheConfig};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Barrier,
    };
    use std::time::Duration;

    /// Cache config matching reth's EpochCacheConfig.
    struct EpochConfig;
    impl CacheConfig for EpochConfig {
        const EPOCHS: bool = true;
    }

    /// A value type with an intentionally slow `Clone` that holds the bucket lock during `get()`.
    ///
    /// In production, the lock hold time depends on the Clone cost of `Option<Account>`.
    /// We use `thread::sleep` to guarantee the lock is held long enough for concurrent
    /// `insert()` calls to observe `LOCKED_BIT` and silently drop the write.
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

    /// Demonstrates that `insert()` is silently dropped when `get()` holds the bucket lock.
    ///
    /// Strategy:
    /// - Multiple reader threads continuously call get() on the same key
    /// - get() holds the bucket lock for 5ms per call (via SlowCloneValue::clone)
    /// - With enough readers, the bucket lock is held almost continuously
    /// - The writer thread attempts multiple inserts, each of which may silently fail
    /// - After stopping readers, we verify the final value
    ///
    /// This exactly mirrors the production bug:
    /// - Readers = multiproof workers calling cache.get() to read state
    /// - Writer = insert_state() updating nonce after block execution
    /// - Silent drop = nonce update lost → stale nonce → receipt root mismatch
    #[test]
    fn fixed_cache_insert_silently_dropped_under_contention() {
        type BH = std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>;

        let num_reader_threads = 4;

        let cache: Arc<Cache<u64, SlowCloneValue, BH, EpochConfig>> =
            Arc::new(Cache::new(4096, Default::default()));

        // Initial state: nonce = 0
        cache.insert(1, SlowCloneValue { nonce: 0 });

        let stop = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(num_reader_threads + 1));

        // Spawn reader threads that continuously get() the same key.
        // Each get() holds the bucket lock for 5ms (the clone duration).
        // With 4 readers, the lock is held ~continuously because as one
        // reader releases, another is already waiting to acquire.
        let readers: Vec<_> = (0..num_reader_threads)
            .map(|_| {
                let c = cache.clone();
                let s = stop.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    while !s.load(Ordering::Relaxed) {
                        // get() acquires bucket lock, clones (sleeps 5ms), releases
                        let _ = c.get(&1);
                    }
                })
            })
            .collect();

        barrier.wait();

        // Give readers time to start and saturate the lock
        std::thread::sleep(Duration::from_millis(10));

        // Writer: attempt many inserts while readers hold the lock.
        // Each insert that lands during a reader's clone() will be silently dropped.
        let total_inserts = 100u64;

        for nonce in 1..=total_inserts {
            cache.insert(1, SlowCloneValue { nonce });
            // Small gap between inserts to allow readers to re-acquire the lock
            std::thread::yield_now();
        }

        // Stop readers and wait for them to finish
        stop.store(true, Ordering::Relaxed);
        for r in readers {
            r.join().unwrap();
        }

        // Check: did the LAST write persist?
        // No readers are active now, so this get() is uncontested.
        let final_nonce = cache.get(&1).unwrap().nonce;

        // Count how many of our writes were silently dropped.
        // If the cache has the latest nonce (total_inserts), all writes succeeded.
        // If it has an earlier nonce, some trailing writes were dropped.
        // If it has 0, ALL writes were dropped.
        let is_stale = final_nonce != total_inserts;

        eprintln!(
            "\n=== FixedCache Race Condition PoC ===\n\
             Reader threads: {num_reader_threads}\n\
             Clone hold:     5ms\n\
             Insert attempts: {total_inserts}\n\
             Final nonce:    {final_nonce} (expected: {total_inserts})\n\
             Write dropped:  {is_stale}\n\
             \n\
             If the final nonce is less than {total_inserts}, it means some insert()\n\
             calls were silently dropped because a reader held the bucket lock.\n\
             In production, this causes the next block to execute with a\n\
             stale nonce, producing a receipt root mismatch and node stall.\n",
        );

        assert!(
            is_stale,
            "Expected the final nonce to be less than {total_inserts} (was {final_nonce}), \
             indicating at least one silently dropped insert. \
             The last successful write was nonce={final_nonce}, meaning all inserts \
             after that were silently dropped by try_lock_ret(None)."
        );
    }
}
