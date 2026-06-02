//! Long-lived blocking thread pool for warming the BAL read-set into the execution cache.
//!
//! The default BAL prewarm runs on the rayon prewarming pool. Rayon is a CPU work-stealing
//! scheduler: when its workers have no stealable work they spin (`sched_yield`) rather than sleep.
//! BAL prewarming is blocking-I/O-bound (MDBX reads), so most workers sit idle and spin, burning
//! CPU that contends with the block executor's threads and slows execution.
//!
//! This pool instead uses dedicated OS threads that **block** on a work queue when idle, so they
//! cost nothing when there's no work and leave the cores for execution. Threads are long-lived
//! (one set per process), cache the state provider across reads of the same block, and release the
//! provider (and its MDBX read txn) when idle so a reader doesn't pin the freelist across blocks.

use alloy_primitives::{Address, StorageKey};
use reth_execution_cache::{CachedStateProvider, ExecutionCache};
use reth_provider::{
    AccountReader, BytecodeReader, ProviderResult, StateProvider, StateProviderBox,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    thread::JoinHandle,
    time::Duration,
};
use tracing::trace;

/// Builds a fresh `StateProviderBox` over the block's parent state. Type-erased so the pool is
/// not generic over the provider factory; a fresh one is passed per block.
type BuildProviderFn = dyn Fn() -> ProviderResult<StateProviderBox> + Send + Sync;

/// A single warm request: a whole account (basic account + its bytecode) or one storage slot.
enum PrewarmTarget {
    Account(Address),
    Storage(Address, StorageKey),
}

struct PrewarmOp {
    /// Monotonic per-block tag. A thread (re)builds its provider when this changes, and skips ops
    /// older than the block it's currently serving (stale leftovers from a finished block).
    epoch: u64,
    build: Arc<BuildProviderFn>,
    caches: ExecutionCache,
    target: PrewarmTarget,
}

/// Long-lived pool of blocking threads that warm the BAL read-set into the shared execution cache.
pub(crate) struct BalPrewarmPool {
    tx: crossbeam_channel::Sender<PrewarmOp>,
    epoch: AtomicU64,
    _handles: Vec<JoinHandle<()>>,
}

impl BalPrewarmPool {
    fn new(num_threads: usize) -> Arc<Self> {
        let (tx, rx) = crossbeam_channel::unbounded::<PrewarmOp>();
        let handles = (0..num_threads)
            .map(|i| {
                let rx = rx.clone();
                std::thread::Builder::new()
                    .name(format!("bal-prewarm-{i}"))
                    .spawn(move || prewarm_loop(rx))
                    .expect("spawn bal-prewarm thread")
            })
            .collect();
        trace!(target: "engine::tree::bal_prewarm_pool", num_threads, "BalPrewarmPool spawned");
        Arc::new(Self { tx, epoch: AtomicU64::new(0), _handles: handles })
    }

    /// Starts a new block's warming epoch. Pass the returned value to every `warm_*` call for the
    /// block so threads rebuild their provider once and ignore stale ops from the previous block.
    pub(crate) fn next_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Fire-and-forget: warm an account (basic account + bytecode) into `caches`.
    pub(crate) fn warm_account(
        &self,
        epoch: u64,
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        addr: Address,
    ) {
        let _ = self.tx.send(PrewarmOp { epoch, build, caches, target: PrewarmTarget::Account(addr) });
    }

    /// Fire-and-forget: warm one storage slot into `caches`.
    pub(crate) fn warm_storage(
        &self,
        epoch: u64,
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        addr: Address,
        slot: StorageKey,
    ) {
        let _ = self
            .tx
            .send(PrewarmOp { epoch, build, caches, target: PrewarmTarget::Storage(addr, slot) });
    }
}

/// Returns the process-global pool, spawning it on first use with [`bal_prewarm_threads`] threads.
pub(crate) fn global() -> Arc<BalPrewarmPool> {
    static POOL: OnceLock<Arc<BalPrewarmPool>> = OnceLock::new();
    POOL.get_or_init(|| BalPrewarmPool::new(bal_prewarm_threads())).clone()
}

/// Number of warming threads, overridable via `RETH_BAL_PREWARM_THREADS`. Defaults to 2× available
/// parallelism: the work is blocking-I/O-bound, so over-subscription raises in-flight SSD requests,
/// and idle threads block (cost nothing) rather than spin.
fn bal_prewarm_threads() -> usize {
    std::env::var("RETH_BAL_PREWARM_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get() * 2).unwrap_or(16))
}

fn prewarm_loop(rx: crossbeam_channel::Receiver<PrewarmOp>) {
    // Release the cached provider (and its MDBX read txn) after this long idle, so a reader doesn't
    // pin MDBX's freelist across the inter-block gap. Within a block, ops arrive far more often.
    const IDLE_RELEASE: Duration = Duration::from_millis(50);

    let mut current: Option<(u64, CachedStateProvider<StateProviderBox>)> = None;
    loop {
        let op = match rx.recv_timeout(IDLE_RELEASE) {
            Ok(op) => op,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                current = None; // idle: drop the read txn, then block for the next block's work
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        // Skip ops from a block we've already moved past (avoids rebuilding the provider backwards).
        if current.as_ref().is_some_and(|(e, _)| op.epoch < *e) {
            continue;
        }

        // (Re)build the provider on a new block.
        if current.as_ref().is_none_or(|(e, _)| *e != op.epoch) {
            match (op.build)() {
                Ok(inner) => {
                    current = Some((op.epoch, CachedStateProvider::new_prewarm(inner, op.caches)));
                }
                Err(err) => {
                    trace!(target: "engine::tree::bal_prewarm_pool", %err, "failed to build provider");
                    current = None;
                    continue;
                }
            }
        }
        let provider = &current.as_ref().expect("just set").1;

        match op.target {
            PrewarmTarget::Account(addr) => {
                if let Ok(Some(account)) = provider.basic_account(&addr) {
                    if let Some(code_hash) = account.bytecode_hash {
                        if code_hash != alloy_consensus::constants::KECCAK_EMPTY {
                            let _ = provider.bytecode_by_hash(&code_hash);
                        }
                    }
                }
            }
            PrewarmTarget::Storage(addr, slot) => {
                let _ = provider.storage(addr, slot);
            }
        }
    }
}
