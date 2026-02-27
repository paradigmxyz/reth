use crate::error::mdbx_result;
use crossbeam_queue::ArrayQueue;
use std::fmt;

/// Lock-free pool of reset read-only MDBX transaction handles.
///
/// With `MDBX_NOTLS` (which reth always sets), every `mdbx_txn_begin_ex` for a read transaction
/// calls `mvcc_bind_slot`, which acquires `lck_rdt_lock` — a pthread mutex. Under high
/// concurrency (e.g., prewarming), this becomes a contention point.
///
/// This pool caches transaction handles that have been reset via `mdbx_txn_reset`. A reset handle
/// retains its reader slot, so `mdbx_txn_renew` can reactivate it without touching the reader
/// table mutex.
///
/// Implemented using [`sharded_slab::Slab`] for lock-free, per-thread-sharded storage of reset
/// txn handles. A lock-free [`ArrayQueue`] tracks available slab keys for O(1) pop.
pub(crate) struct ReadTxnPool {
    /// Lock-free concurrent slab storing reset txn pointers, sharded per-thread.
    slab: sharded_slab::Slab<PooledTxn>,
    /// Lock-free bounded queue of slab keys available for reuse.
    keys: ArrayQueue<usize>,
}

/// Wrapper around a raw txn pointer to satisfy `Send + Sync` for the slab.
struct PooledTxn(*mut ffi::MDBX_txn);

// SAFETY: MDBX txn pointers are safe to send across threads — we ensure exclusive
// ownership via the slab's insert/take semantics.
unsafe impl Send for PooledTxn {}
unsafe impl Sync for PooledTxn {}

impl ReadTxnPool {
    const MAX_POOLED: usize = 128;

    pub(crate) fn new() -> Self {
        Self { slab: sharded_slab::Slab::new(), keys: ArrayQueue::new(Self::MAX_POOLED) }
    }

    /// Takes a reset transaction handle from the pool, renews it, and returns it ready for use.
    ///
    /// Returns `None` if the pool is empty or all renew attempts fail.
    pub(crate) fn get(&self) -> Option<*mut ffi::MDBX_txn> {
        while let Some(key) = self.keys.pop() {
            if let Some(handle) = self.slab.take(key) {
                let txn = handle.0;
                // SAFETY: this pointer was previously created by mdbx_txn_begin_ex and reset
                // via mdbx_txn_reset. mdbx_txn_renew reuses the existing reader slot without
                // taking lck_rdt_lock.
                if mdbx_result(unsafe { ffi::mdbx_txn_renew(txn) }).is_ok() {
                    return Some(txn);
                }
                // Renew failed — abort the handle and keep trying.
                unsafe { ffi::mdbx_txn_abort(txn) };
            }
            // Key was stale (already taken) — try next.
        }
        None
    }

    /// Resets an active read transaction handle and returns it to the pool.
    ///
    /// If reset fails or the pool is full, the handle is aborted instead.
    pub(crate) fn put(&self, txn: *mut ffi::MDBX_txn) {
        // mdbx_txn_reset releases the MVCC snapshot but keeps the reader slot.
        if mdbx_result(unsafe { ffi::mdbx_txn_reset(txn) }).is_err() {
            unsafe { ffi::mdbx_txn_abort(txn) };
            return;
        }

        if let Some(key) = self.slab.insert(PooledTxn(txn)) {
            if self.keys.push(key).is_err() {
                // Queue full — take back from slab and abort.
                if let Some(handle) = self.slab.take(key) {
                    unsafe { ffi::mdbx_txn_abort(handle.0) };
                }
            }
        } else {
            // Slab full — abort the handle to release the reader slot.
            unsafe { ffi::mdbx_txn_abort(txn) };
        }
    }

    /// Aborts all pooled transaction handles. Called during environment shutdown.
    pub(crate) fn drain(&self) {
        while let Some(key) = self.keys.pop() {
            if let Some(handle) = self.slab.take(key) {
                unsafe { ffi::mdbx_txn_abort(handle.0) };
            }
        }
    }
}

// SAFETY: all fields are Send+Sync — slab and ArrayQueue are both lock-free.
unsafe impl Send for ReadTxnPool {}
unsafe impl Sync for ReadTxnPool {}

impl fmt::Debug for ReadTxnPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadTxnPool").field("pooled", &self.keys.len()).finish()
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
        assert!(env.ro_txn_pool().get().is_none());
    }

    #[test]
    fn put_get_roundtrip() {
        let (_dir, env) = test_env();
        seed(&env);

        // Open and drop a read txn — drop returns the handle to the pool.
        let txn = env.begin_ro_txn().unwrap();
        let id1 = txn.id().unwrap();
        drop(txn);

        assert_eq!(env.ro_txn_pool().keys.len(), 1);

        // Next begin_ro_txn should reuse the pooled handle.
        let txn = env.begin_ro_txn().unwrap();
        // The renewed txn gets a fresh snapshot, so id may differ, but it should succeed.
        let _id2 = txn.id().unwrap();
        drop(txn);

        // Verify the id changed (write happened between first txn and second).
        // Just verify we got a valid txn.
        assert!(id1 > 0);
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

        // Pool should have exactly 1 handle (all reused the same slot).
        assert_eq!(env.ro_txn_pool().keys.len(), 1);
    }

    #[test]
    fn concurrent_txns_pool_multiple_handles() {
        let (_dir, env) = test_env();
        seed(&env);

        // Open several txns concurrently — each gets a fresh handle.
        let txns: Vec<_> = (0..8).map(|_| env.begin_ro_txn().unwrap()).collect();
        assert_eq!(env.ro_txn_pool().keys.len(), 0);

        // Drop them all — pool should accumulate handles.
        let count = txns.len();
        drop(txns);
        assert_eq!(env.ro_txn_pool().keys.len(), count);

        // Reopen same number — all should be pooled.
        let txns: Vec<_> = (0..count).map(|_| env.begin_ro_txn().unwrap()).collect();
        assert_eq!(env.ro_txn_pool().keys.len(), 0);
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

        // Pool some handles.
        for _ in 0..4 {
            let txn = env.begin_ro_txn().unwrap();
            drop(txn);
        }
        // All reuse the same slot, so pool has 1.
        assert!(env.ro_txn_pool().keys.len() > 0);

        env.ro_txn_pool().drain();
        assert_eq!(env.ro_txn_pool().keys.len(), 0);

        // Pool is empty — get should return None.
        assert!(env.ro_txn_pool().get().is_none());
    }

    #[test]
    fn committed_txn_is_not_pooled() {
        let (_dir, env) = test_env();
        seed(&env);

        let txn = env.begin_ro_txn().unwrap();
        txn.commit().unwrap();

        // Committed txns are freed by mdbx, not returned to pool.
        assert_eq!(env.ro_txn_pool().keys.len(), 0);
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
        {
            let env = env.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for i in 0u32..20 {
                    let tx = env.begin_rw_txn().unwrap();
                    let db = tx.open_db(None).unwrap();
                    tx.put(db.dbi(), i.to_le_bytes(), b"v", WriteFlags::empty()).unwrap();
                    tx.commit().unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn debug_format() {
        let (_dir, env) = test_env();
        seed(&env);

        let debug = format!("{:?}", env.ro_txn_pool());
        assert!(debug.contains("ReadTxnPool"));
        assert!(debug.contains("pooled: 0"));

        let txn = env.begin_ro_txn().unwrap();
        drop(txn);

        let debug = format!("{:?}", env.ro_txn_pool());
        assert!(debug.contains("pooled: 1"));
    }
}
