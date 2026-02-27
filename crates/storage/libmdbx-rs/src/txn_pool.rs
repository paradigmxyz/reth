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
