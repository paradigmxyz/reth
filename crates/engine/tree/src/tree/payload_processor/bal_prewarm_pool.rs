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
use tokio::sync::oneshot;
use tracing::trace;

/// Builds a fresh `StateProviderBox` over the block's parent state. Type-erased so the pool is not
/// generic over the provider factory; each worker builds its own per block.
type BuildProviderFn = dyn Fn() -> ProviderResult<StateProviderBox> + Send + Sync;

/// A single warm request: a whole account (basic account + its bytecode) or one storage slot.
pub(crate) enum PrewarmTarget {
    /// Warm an account's basic account data and bytecode.
    Account(Address),
    /// Warm one storage slot.
    Storage(Address, StorageKey),
}

/// A message in a worker's queue. The per-block lifecycle is explicit and ordered (the queue is
/// FIFO): one `BeginBlock`, then the worker's share of `Warm`s, then one `EndBlock`.
enum PrewarmMsg {
    /// Open a read txn for the new block: build a provider over the parent state and hold it.
    BeginBlock { build: Arc<BuildProviderFn>, caches: ExecutionCache },
    /// Warm a batch of targets into the held provider's cache. Ignored if no provider is held.
    ///
    /// Targets are batched so workers spend their time reading state instead of cycling through
    /// the channel receive path (spin, yield, park) for every single slot.
    Warm(Vec<PrewarmTarget>),
    /// Drop the held provider (and its read txn).
    EndBlock(Arc<SendOnDrop>),
}

/// Long-lived pool of blocking threads that warm the BAL read-set into the shared execution cache.
#[derive(Debug)]
pub(crate) struct BalPrewarmPool {
    /// One queue per worker. `BeginBlock`/`EndBlock` are broadcast to all; `Warm`s round-robin.
    workers: Vec<crossbeam_channel::Sender<PrewarmMsg>>,
    /// Round-robin cursor for distributing warm batches across workers.
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

    /// Fire-and-forget: warm a batch of targets on some worker.
    pub(crate) fn warm_batch(&self, batch: Vec<PrewarmTarget>) {
        if batch.is_empty() {
            return;
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let _ = self.workers[i].send(PrewarmMsg::Warm(batch));
    }

    /// Ends the block: every worker drops its provider (and read txn) once it has drained the warm
    /// requests queued ahead of this message.
    ///
    /// Blocks until all workers processed the end block message.
    pub(crate) fn end_block(&self) {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(SendOnDrop { sender: Some(tx) });

        for worker in &self.workers {
            let _ = worker.send(PrewarmMsg::EndBlock(tx.clone()));
        }

        drop(tx);
        rx.blocking_recv().expect("BAL prewarm pool dropped without signaling completion");
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
/// The prewarm pool also runs concurrently with the parallel execution workers, whose own reads
/// (bytecode loads and keys not served by the BAL) go to the same MDBX map. Profiles of warm-cache
/// big-block runs show execution workers mostly off-CPU while a large prewarm pool saturates the
/// scheduler and the page-cache locks, so the pool is sized to leave the execution workers room
/// while still keeping a reasonable `NVMe` queue depth for cold pages.
pub(crate) const DEFAULT_BAL_PREWARM_THREADS: usize = 32;

/// Number of targets per warm batch.
///
/// Batches amortize the per-message channel overhead while staying small enough that the
/// round-robin distribution keeps all workers busy.
pub(crate) const PREWARM_BATCH_SIZE: usize = 64;

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
            PrewarmMsg::Warm(batch) => {
                let Some(provider) = provider.as_ref() else { continue };
                for target in batch {
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
            }
            PrewarmMsg::EndBlock(end_tx) => {
                provider = None;
                drop(end_tx);
            }
        }
    }
}

struct SendOnDrop {
    sender: Option<oneshot::Sender<()>>,
}

impl Drop for SendOnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
    }
}
