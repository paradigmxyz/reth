use crate::error::mdbx_result;
use crossbeam_queue::ArrayQueue;

/// Lock-free pool of reset read-only MDBX transaction handles.
///
/// With `MDBX_NOTLS` (which reth always sets), every `mdbx_txn_begin_ex` for a read transaction
/// calls `mvcc_bind_slot`, which acquires `lck_rdt_lock` — a pthread mutex. Under high
/// concurrency (e.g., prewarming), this becomes a contention point.
///
/// This pool caches transaction handles that have been reset via `mdbx_txn_reset`. A reset handle
/// retains its reader slot, so `mdbx_txn_renew` can reactivate it without touching the reader
/// table mutex.
pub(crate) struct ReadTxnPool {
    queue: ArrayQueue<PooledTxn>,
}

/// Wrapper around a raw txn pointer to satisfy `Send + Sync` for the queue.
struct PooledTxn(*mut ffi::MDBX_txn);

// SAFETY: MDBX txn pointers are safe to send across threads — we ensure exclusive
// ownership via the queue's push/pop semantics.
unsafe impl Send for PooledTxn {}
unsafe impl Sync for PooledTxn {}

impl ReadTxnPool {
    pub(crate) fn new() -> Self {
        Self { queue: ArrayQueue::new(256) }
    }

    /// Takes a reset transaction handle from the pool, renews it, and returns it ready for use.
    ///
    /// Returns `None` if the pool is empty or all renew attempts fail.
    pub(crate) fn pop(&self) -> Option<*mut ffi::MDBX_txn> {
        while let Some(handle) = self.queue.pop() {
            let txn = handle.0;
            // SAFETY: this pointer was previously created by mdbx_txn_begin_ex and reset
            // via mdbx_txn_reset. mdbx_txn_renew reuses the existing reader slot without
            // taking lck_rdt_lock.
            match mdbx_result(unsafe { ffi::mdbx_txn_renew(txn) }) {
                Ok(_) => return Some(txn),
                Err(e) => {
                    tracing::warn!(target: "libmdbx", %e, "failed to renew pooled read transaction");
                    abort_txn(txn);
                }
            }
        }
        None
    }

    /// Resets an active read transaction handle and returns it to the pool.
    ///
    /// If reset fails, the handle is aborted instead.
    pub(crate) fn push(&self, txn: *mut ffi::MDBX_txn) {
        // mdbx_txn_reset releases the MVCC snapshot but keeps the reader slot.
        if let Err(e) = mdbx_result(unsafe { ffi::mdbx_txn_reset(txn) }) {
            tracing::warn!(target: "libmdbx", %e, "failed to reset read transaction for pooling");
            abort_txn(txn);
            return;
        }

        if self.queue.push(PooledTxn(txn)).is_err() {
            abort_txn(txn);
        }
    }

    /// Aborts all pooled transaction handles. Called during environment shutdown.
    pub(crate) fn drain(&self) {
        while let Some(handle) = self.queue.pop() {
            abort_txn(handle.0);
        }
    }
}

/// Aborts a transaction handle, logging any error.
fn abort_txn(txn: *mut ffi::MDBX_txn) {
    if let Err(e) = mdbx_result(unsafe { ffi::mdbx_txn_abort(txn) }) {
        tracing::error!(target: "libmdbx", %e, "failed to abort transaction");
    }
}

impl Drop for ReadTxnPool {
    fn drop(&mut self) {
        self.drain();
    }
}

#[cfg(test)]
mod tests {
    use crate::{Environment, WriteFlags};

    /// Opens a fresh test environment.
    fn test_env() -> (tempfile::TempDir, Environment) {
        let dir = tempfile::tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();
        (dir, env)
    }

    /// Inserts a single key so the database isn't empty.
    fn seed(env: &Environment) {
        let tx = env.begin_rw_txn().unwrap();
        let db = tx.open_db(None).unwrap();
        tx.put(db.dbi(), b"key", b"val", WriteFlags::empty()).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn get_returns_none_when_empty() {
        let (_dir, env) = test_env();
        assert!(env.ro_txn_pool().pop().is_none());
    }

    #[test]
    fn put_get_roundtrip() {
        let (_dir, env) = test_env();
        seed(&env);

        // Open and drop a read txn — drop returns the handle to the pool.
        let txn = env.begin_ro_txn().unwrap();
        drop(txn);

        // Next begin_ro_txn should reuse the pooled handle.
        let txn = env.begin_ro_txn().unwrap();
        let _id = txn.id().unwrap();
    }

    #[test]
    fn pooled_txn_reads_latest_snapshot() {
        let (_dir, env) = test_env();
        seed(&env);

        // Open a read txn and drop it to pool the handle.
        let txn = env.begin_ro_txn().unwrap();
        drop(txn);

        // Write new data.
        {
            let tx = env.begin_rw_txn().unwrap();
            let db = tx.open_db(None).unwrap();
            tx.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
            tx.commit().unwrap();
        }

        // The renewed pooled txn must see the new data.
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let val: Option<[u8; 4]> = txn.get(db.dbi(), b"key2").unwrap();
        assert_eq!(val, Some(*b"val2"));
    }

    #[test]
    fn multiple_put_get_cycles() {
        let (_dir, env) = test_env();
        seed(&env);

        for _ in 0..50 {
            let txn = env.begin_ro_txn().unwrap();
            let db = txn.open_db(None).unwrap();
            let val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
            assert_eq!(val, Some(*b"val"));
            drop(txn);
        }
    }

    #[test]
    fn concurrent_txns_pool_multiple_handles() {
        let (_dir, env) = test_env();
        seed(&env);

        // Open several txns concurrently — each gets a fresh handle.
        let txns: Vec<_> = (0..8).map(|_| env.begin_ro_txn().unwrap()).collect();

        // Drop them all — pool should accumulate handles.
        let count = txns.len();
        drop(txns);

        // Reopen same number — all should come from the pool.
        let txns: Vec<_> = (0..count).map(|_| env.begin_ro_txn().unwrap()).collect();
        for txn in &txns {
            let db = txn.open_db(None).unwrap();
            let val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
            assert_eq!(val, Some(*b"val"));
        }
    }

    #[test]
    fn drain_empties_pool() {
        let (_dir, env) = test_env();
        seed(&env);

        // Pool a handle.
        let txn = env.begin_ro_txn().unwrap();
        drop(txn);

        env.ro_txn_pool().drain();

        // Pool is empty — get should return None.
        assert!(env.ro_txn_pool().pop().is_none());
    }

    #[test]
    fn committed_txn_is_not_pooled() {
        let (_dir, env) = test_env();
        seed(&env);

        let txn = env.begin_ro_txn().unwrap();
        txn.commit().unwrap();

        // Committed txns are freed by mdbx, not returned to pool.
        assert!(env.ro_txn_pool().pop().is_none());
    }

    #[test]
    fn multithreaded_pool_usage() {
        let (_dir, env) = test_env();
        seed(&env);

        let env = std::sync::Arc::new(env);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let env = env.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..100 {
                        let txn = env.begin_ro_txn().unwrap();
                        let db = txn.open_db(None).unwrap();
                        let val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
                        assert_eq!(val, Some(*b"val"));
                        drop(txn);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn multithreaded_mixed_read_write() {
        let (_dir, env) = test_env();
        seed(&env);

        let env = std::sync::Arc::new(env);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));

        // Spawn reader threads.
        let mut handles: Vec<_> = (0..4)
            .map(|_| {
                let env = env.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..50 {
                        let txn = env.begin_ro_txn().unwrap();
                        let db = txn.open_db(None).unwrap();
                        // key may or may not exist depending on writer timing.
                        let _val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
                        drop(txn);
                    }
                })
            })
            .collect();

        // Spawn a writer thread.
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for i in 0u32..20 {
                let tx = env.begin_rw_txn().unwrap();
                let db = tx.open_db(None).unwrap();
                tx.put(db.dbi(), i.to_le_bytes(), b"v", WriteFlags::empty()).unwrap();
                tx.commit().unwrap();
            }
        }));

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn multithreaded_concurrent_open_close() {
        let (_dir, env) = test_env();
        seed(&env);

        let env = std::sync::Arc::new(env);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));

        // 16 threads each open and close 200 txns — exercises pool contention.
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let env = env.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..200 {
                        let txn = env.begin_ro_txn().unwrap();
                        let db = txn.open_db(None).unwrap();
                        let val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
                        assert_eq!(val, Some(*b"val"));
                        // Intentionally don't call drop explicitly — let scope handle it.
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn multithreaded_hold_multiple_txns() {
        let (_dir, env) = test_env();
        seed(&env);

        let env = std::sync::Arc::new(env);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        // Each thread holds multiple txns open simultaneously, then drops them all.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let env = env.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..20 {
                        let txns: Vec<_> = (0..4).map(|_| env.begin_ro_txn().unwrap()).collect();
                        for txn in &txns {
                            let db = txn.open_db(None).unwrap();
                            let val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
                            assert_eq!(val, Some(*b"val"));
                        }
                        drop(txns);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn multithreaded_drain_under_contention() {
        let (_dir, env) = test_env();
        seed(&env);

        let env = std::sync::Arc::new(env);

        // Fill the pool with handles.
        {
            let txns: Vec<_> = (0..16).map(|_| env.begin_ro_txn().unwrap()).collect();
            drop(txns);
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));

        // 4 threads racing to get from pool + 1 thread draining.
        let mut handles: Vec<_> = (0..4)
            .map(|_| {
                let env = env.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..50 {
                        let _txn = env.begin_ro_txn(); // may or may not get a pooled handle
                    }
                })
            })
            .collect();

        handles.push(std::thread::spawn(move || {
            barrier.wait();
            env.ro_txn_pool().drain();
        }));

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn pool_overflow_aborts_excess() {
        let dir = tempfile::tempdir().unwrap();
        let env = Environment::builder().set_max_readers(512).open(dir.path()).unwrap();
        seed(&env);

        // Open more txns than the pool capacity (256), drop them all.
        let txns: Vec<_> = (0..300).map(|_| env.begin_ro_txn().unwrap()).collect();
        drop(txns);

        // Pool is capped at 256; excess handles are aborted.
        assert_eq!(env.ro_txn_pool().queue.len(), 256);

        // All 256 pooled handles should still work.
        let txns: Vec<_> = (0..256).map(|_| env.begin_ro_txn().unwrap()).collect();
        for txn in &txns {
            let db = txn.open_db(None).unwrap();
            let val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
            assert_eq!(val, Some(*b"val"));
        }
    }
}
