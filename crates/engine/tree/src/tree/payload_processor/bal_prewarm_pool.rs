use alloy_primitives::{Address, StorageKey};
use reth_execution_cache::{CachedStateProvider, ExecutionCache};
use reth_provider::{
    AccountReader, BytecodeReader, ProviderResult, StateProvider, StateProviderBox,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::JoinHandle,
};
use tracing::trace;

/// Builds a fresh `StateProviderBox` over the block's parent state. Type-erased so the pool is not
/// generic over the provider factory; each worker builds its own per block.
type BuildProviderFn = dyn Fn() -> ProviderResult<StateProviderBox> + Send + Sync;

/// A single warm request: a whole account (basic account + its bytecode) or one storage slot.
enum PrewarmTarget {
    Account(Address),
    Storage(Address, StorageKey),
}

/// A message in a worker's queue. The per-block lifecycle is explicit and ordered (the queue is
/// FIFO): one `BeginBlock`, then the worker's share of `Warm`s, then one `EndBlock`.
enum PrewarmMsg {
    /// Open a read txn for the new block: build a provider over the parent state and hold it.
    BeginBlock { build: Arc<BuildProviderFn>, caches: ExecutionCache },
    /// Warm one target into the held provider's cache. Ignored if no provider is held.
    Warm(PrewarmTarget),
    /// Drop the held provider (and its read txn).
    EndBlock,
}

/// Long-lived pool of blocking threads that warm the BAL read-set into the shared execution cache.
#[derive(Debug)]
pub(crate) struct BalPrewarmPool {
    /// One queue per worker. `BeginBlock`/`EndBlock` are broadcast to all; `Warm`s round-robin.
    workers: Vec<crossbeam_channel::Sender<PrewarmMsg>>,
    /// Round-robin cursor for distributing warm requests across workers.
    next: AtomicUsize,
    _handles: Vec<JoinHandle<()>>,
}

impl BalPrewarmPool {
    /// Spawns `num_threads` long-lived blocking worker threads. Owned by the
    /// [`PayloadProcessor`](super::PayloadProcessor); the threads exit when the pool is dropped.
    pub(crate) fn new(num_threads: usize) -> Arc<Self> {
        let mut workers = Vec::with_capacity(num_threads);
        let mut handles = Vec::with_capacity(num_threads);
        for i in 0..num_threads {
            let (tx, rx) = crossbeam_channel::unbounded::<PrewarmMsg>();
            workers.push(tx);
            handles.push(
                std::thread::Builder::new()
                    .name(format!("bal-prewarm-{i:03}"))
                    .spawn(move || prewarm_loop(rx))
                    .expect("spawn bal-prewarm thread"),
            );
        }
        trace!(target: "engine::tree::bal_prewarm_pool", num_threads, "BalPrewarmPool spawned");
        Arc::new(Self { workers, next: AtomicUsize::new(0), _handles: handles })
    }

    /// Begins a block: hands every worker the provider builder and shared cache so each opens its
    /// own read txn over the parent state. Pair with [`end_block`](Self::end_block).
    pub(crate) fn begin_block(&self, build: Arc<BuildProviderFn>, caches: ExecutionCache) {
        for worker in &self.workers {
            let _ = worker
                .send(PrewarmMsg::BeginBlock { build: build.clone(), caches: caches.clone() });
        }
    }

    /// Fire-and-forget: warm an account (basic account + bytecode) on some worker.
    pub(crate) fn warm_account(&self, addr: Address) {
        self.send_warm(PrewarmTarget::Account(addr));
    }

    /// Fire-and-forget: warm one storage slot on some worker.
    pub(crate) fn warm_storage(&self, addr: Address, slot: StorageKey) {
        self.send_warm(PrewarmTarget::Storage(addr, slot));
    }

    /// Ends the block: every worker drops its provider (and read txn) once it has drained the warm
    /// requests queued ahead of this message.
    pub(crate) fn end_block(&self) {
        for worker in &self.workers {
            let _ = worker.send(PrewarmMsg::EndBlock);
        }
    }

    fn send_warm(&self, target: PrewarmTarget) {
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let _ = self.workers[i].send(PrewarmMsg::Warm(target));
    }
}

/// Number of warming threads.
///
/// The work performed on those threads boils down mostly to MDBX reads. An MDBX read consists of
/// a tree traversal and major page faults causing I/O.
///
/// In order to utilize the parallelism of `NVMe` we have to give it enough work, or equally,
/// maintain a high queue depth. Modern `NVMe` devices require in between 64-128 requests in-flight
/// to achieve its peak performance. Ideally we don't grow past that but it's OK to do so, it just
/// means that a request is going to wait in the `NVMe` queue rather than in memory.
///
/// MDBX piggy-backs on the OS page cache for its buffers. Oftentimes, the hit rate reaches 90-99%
/// hit rate. At that point, the workload can be classified as CPU-bound. In that case, having
/// a high number of threads is counterproductive due to the effects of context switching, core
/// migration, contention, etc.
///
/// However, that overhead is considered negligible compared to the benefits of utilizing `NVMe`
/// resources. For example, with request latency of 100µs, 100k IO requests the expected time to
/// finish is 312.5ms at QD=32 and 156.26ms at QD=64. Use the lower end of the useful range so BAL
/// prewarming does not crowd out proof workers and execution-cache work on the same payload.
///
/// This should explain why this particular value is picked.
pub(crate) const DEFAULT_BAL_PREWARM_THREADS: usize = 64;

fn prewarm_loop(rx: crossbeam_channel::Receiver<PrewarmMsg>) {
    // The provider (and its MDBX read txn) held for the current block, between `BeginBlock` and
    // `EndBlock`. `None` while idle, so no read txn is pinned across the inter-block gap.
    let mut provider: Option<CachedStateProvider<StateProviderBox>> = None;

    // Blocks when idle; the channel disconnects (and the loop ends) when the pool is dropped.
    while let Ok(msg) = rx.recv() {
        match msg {
            PrewarmMsg::BeginBlock { build, caches } => {
                provider = match (build)() {
                    Ok(inner) => Some(CachedStateProvider::new_prewarm(inner, caches)),
                    Err(err) => {
                        trace!(target: "engine::tree::bal_prewarm_pool", %err, "failed to build provider");
                        None
                    }
                };
            }
            PrewarmMsg::Warm(target) => {
                let Some(provider) = provider.as_ref() else { continue };
                match target {
                    PrewarmTarget::Account(addr) => {
                        if let Ok(Some(account)) = provider.basic_account(&addr) &&
                            let Some(code_hash) = account.bytecode_hash &&
                            code_hash != alloy_consensus::constants::KECCAK_EMPTY
                        {
                            let _ = provider.bytecode_by_hash(&code_hash);
                        }
                    }
                    PrewarmTarget::Storage(addr, slot) => {
                        let _ = provider.storage(addr, slot);
                    }
                }
            }
            PrewarmMsg::EndBlock => {
                provider = None;
            }
        }
    }
}
