use crate::error::mdbx_result;
use crossbeam_queue::ArrayQueue;

/// Lock-free pool of reset read-only MDBX transaction handles.
///
/// With `MDBX_NOSTICKYTHREADS` (which reth always sets), every `mdbx_txn_begin_ex` for a read
/// transaction calls `mvcc_bind_slot`, which acquires `lck_rdt_lock` — a pthread mutex. Under high
/// concurrency (e.g., prewarming), this becomes a contention point.
///
/// This pool caches transaction handles that have been reset via `mdbx_txn_reset`. A reset handle
/// retains its reader slot, so `mdbx_txn_renew` can reactivate it without touching the reader
/// table mutex.
///
/// On hosts with several last-level-cache (LLC) domains, such as the CCDs of an AMD EPYC, the pool
/// keeps one queue per domain. A thread pushes to and pops from its own domain's queue first, so
/// the queue's head and tail lines and the handles' state stay in that domain's cache instead of
/// bouncing between domains; it steals from other domains only when its own queue is empty.
pub(crate) struct ReadTxnPool {
    /// One queue per LLC domain, indexed by [`llc::current_domain`]. A single queue when the host
    /// has one domain or its topology is unknown.
    queues: Box<[ArrayQueue<PooledTxn>]>,
}

/// Wrapper around a raw txn pointer to satisfy `Send + Sync` for the queue.
struct PooledTxn(*mut ffi::MDBX_txn);

// SAFETY: MDBX txn pointers are safe to send across threads — we ensure exclusive
// ownership via the queue's push/pop semantics.
unsafe impl Send for PooledTxn {}
unsafe impl Sync for PooledTxn {}

impl ReadTxnPool {
    /// Capacity of each domain's queue.
    const QUEUE_CAPACITY: usize = 256;

    pub(crate) fn new() -> Self {
        Self::with_domains(llc::domains())
    }

    fn with_domains(domains: usize) -> Self {
        Self {
            queues: (0..domains.max(1)).map(|_| ArrayQueue::new(Self::QUEUE_CAPACITY)).collect(),
        }
    }

    /// The calling thread's queue index.
    fn local(&self) -> usize {
        if self.queues.len() == 1 {
            return 0
        }
        llc::current_domain().map_or(0, |domain| domain % self.queues.len())
    }

    /// Takes a reset transaction handle from the pool, renews it, and returns it ready for use.
    ///
    /// Tries the calling thread's domain first, then steals from the others.
    ///
    /// Returns `None` if the pool is empty or all renew attempts fail.
    pub(crate) fn pop(&self) -> Option<*mut ffi::MDBX_txn> {
        let local = self.local();
        let n = self.queues.len();
        for queue in (0..n).map(|i| &self.queues[(local + i) % n]) {
            while let Some(handle) = queue.pop() {
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
        }
        None
    }

    /// Resets an active read transaction handle and returns it to the calling thread's domain
    /// queue.
    ///
    /// If reset fails or the queue is full, the handle is aborted instead.
    pub(crate) fn push(&self, txn: *mut ffi::MDBX_txn) {
        // mdbx_txn_reset releases the MVCC snapshot but keeps the reader slot.
        if let Err(e) = mdbx_result(unsafe { ffi::mdbx_txn_reset(txn) }) {
            tracing::warn!(target: "libmdbx", %e, "failed to reset read transaction for pooling");
            abort_txn(txn);
            return;
        }

        if self.queues[self.local()].push(PooledTxn(txn)).is_err() {
            abort_txn(txn);
        }
    }

    /// Aborts all pooled transaction handles. Called during environment shutdown.
    pub(crate) fn drain(&self) {
        for queue in &self.queues {
            while let Some(handle) = queue.pop() {
                abort_txn(handle.0);
            }
        }
    }

    /// Returns the number of pooled handles across all domains.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.queues.iter().map(ArrayQueue::len).sum()
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

/// Last-level-cache topology, read once from sysfs.
///
/// Mirrors the domain detection of `reth_tasks::affinity`, which this crate does not depend on,
/// including its `RETH_CACHE_AFFINITY=0` opt-out.
mod llc {
    use std::sync::OnceLock;

    /// Domain of each CPU, or `None` when its cache topology was unreadable. Empty when the host
    /// has fewer than two domains.
    fn table() -> &'static [Option<u16>] {
        static TABLE: OnceLock<Box<[Option<u16>]>> = OnceLock::new();
        TABLE.get_or_init(detect)
    }

    /// Returns the number of LLC domains, or zero when there are fewer than two.
    pub(super) fn domains() -> usize {
        table().iter().flatten().map(|&domain| domain as usize + 1).max().unwrap_or(0)
    }

    /// Returns the LLC domain of the CPU the calling thread runs on. Advisory: the thread may
    /// migrate right after.
    #[cfg(target_os = "linux")]
    pub(super) fn current_domain() -> Option<usize> {
        // SAFETY: sched_getcpu takes no arguments and returns -1 on error.
        let cpu = usize::try_from(unsafe { libc::sched_getcpu() }).ok()?;
        table().get(cpu).copied().flatten().map(usize::from)
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) const fn current_domain() -> Option<usize> {
        None
    }

    #[cfg(target_os = "linux")]
    fn detect() -> Box<[Option<u16>]> {
        if std::env::var("RETH_CACHE_AFFINITY").is_ok_and(|v| v == "0" || v == "false") {
            return Box::new([])
        }
        let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu") else { return Box::new([]) };
        let mut lists = Vec::new();
        for entry in entries.flatten() {
            if let Some(cpu) = entry
                .file_name()
                .to_string_lossy()
                .strip_prefix("cpu")
                .and_then(|id| id.parse::<usize>().ok()) &&
                let Some(list) = llc_list(&entry.path())
            {
                lists.push((cpu, list));
            }
        }
        from_lists(&lists)
    }

    #[cfg(not(target_os = "linux"))]
    fn detect() -> Box<[Option<u16>]> {
        Box::new([])
    }

    /// The `shared_cpu_list` of the CPU's highest-level Data or Unified cache leaf.
    #[cfg(target_os = "linux")]
    fn llc_list(cpu_dir: &std::path::Path) -> Option<String> {
        let mut best: Option<(u32, String)> = None;
        for entry in std::fs::read_dir(cpu_dir.join("cache")).ok()?.flatten() {
            let leaf = entry.path();
            let read = |name: &str| std::fs::read_to_string(leaf.join(name)).ok();
            if !entry.file_name().to_string_lossy().starts_with("index") ||
                !read("type").is_some_and(|kind| matches!(kind.trim(), "Data" | "Unified"))
            {
                continue
            }
            if let Some(level) = read("level").and_then(|level| level.trim().parse::<u32>().ok()) &&
                let Some(list) = read("shared_cpu_list").map(|list| list.trim().to_string()) &&
                !list.is_empty() &&
                best.as_ref().is_none_or(|(best, _)| level > *best)
            {
                best = Some((level, list));
            }
        }
        best.map(|(_, list)| list)
    }

    /// Builds the CPU-to-domain table from `(cpu, shared_cpu_list)` pairs, one domain per distinct
    /// list. Returns an empty table for fewer than two domains or all-private caches.
    #[cfg(any(test, target_os = "linux"))]
    pub(super) fn from_lists(lists: &[(usize, String)]) -> Box<[Option<u16>]> {
        let Some(max_cpu) = lists.iter().map(|(cpu, _)| *cpu).max() else { return Box::new([]) };
        let mut table = vec![None; max_cpu + 1];
        let mut domains = Vec::<&str>::new();
        for (cpu, list) in lists {
            let domain = domains.iter().position(|d| d == list).unwrap_or_else(|| {
                domains.push(list);
                domains.len() - 1
            });
            let Ok(domain) = u16::try_from(domain) else { return Box::new([]) };
            table[*cpu] = Some(domain);
        }
        if domains.len() < 2 || domains.len() == lists.len() {
            return Box::new([])
        }
        table.into_boxed_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn llc_table_from_lists() {
        let lists = [(0, "0-1".to_string()), (1, "0-1".to_string()), (3, "2-3".to_string())];
        assert_eq!(&*llc::from_lists(&lists), &[Some(0), Some(0), None, Some(1)]);
        assert!(llc::from_lists(&lists[..2]).is_empty());
        assert!(llc::from_lists(&[]).is_empty());
    }

    #[test]
    fn pop_steals_from_other_domains() {
        let (_dir, env) = test_env();
        seed(&env);
        let pool = ReadTxnPool::with_domains(4);

        // Take a live handle from the environment's pool and queue it in a domain other than the
        // calling thread's.
        drop(env.begin_ro_txn().unwrap());
        let ptr = env.ro_txn_pool().pop().unwrap();
        // SAFETY: the handle is an active read txn; resetting it keeps its reader slot.
        mdbx_result(unsafe { ffi::mdbx_txn_reset(ptr) }).unwrap();
        let other = (pool.local() + 1) % pool.queues.len();
        assert!(pool.queues[other].push(PooledTxn(ptr)).is_ok());

        assert_eq!(pool.pop(), Some(ptr));
        assert_eq!(pool.len(), 0);
        pool.push(ptr);
        assert_eq!(pool.len(), 1);
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
        assert_eq!(env.ro_txn_pool().len(), 256);

        // All 256 pooled handles should still work.
        let txns: Vec<_> = (0..256).map(|_| env.begin_ro_txn().unwrap()).collect();
        for txn in &txns {
            let db = txn.open_db(None).unwrap();
            let val: Option<[u8; 3]> = txn.get(db.dbi(), b"key").unwrap();
            assert_eq!(val, Some(*b"val"));
        }
    }
}
