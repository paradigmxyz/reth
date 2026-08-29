use alloy_primitives::{Address, StorageKey};
use reth_execution_cache::{CachedStateProvider, ExecutionCache, TxPoolPrewarmCacheSnapshot};
use reth_provider::{
    AccountReader, BytecodeReader, ProviderResult, StateProvider, StateProviderBox,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
enum PrewarmTarget {
    Account(Address),
    Storage(Address, StorageKey),
}

/// A message in a worker's queue. The per-block lifecycle is explicit and ordered (the queue is
/// FIFO): one `BeginBlock`, then the worker's share of `Warm`s, then one `EndBlock`.
enum PrewarmMsg {
    /// Open a read txn for the new block: build a provider over the parent state and hold it.
    BeginBlock {
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
        cancelled: Arc<AtomicBool>,
    },
    /// Warm one target into the held provider's cache. Ignored if no provider is held.
    Warm(PrewarmTarget),
    /// Drop the held provider (and its read txn).
    EndBlock(Arc<SendOnDrop>),
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
    pub(crate) fn begin_block(
        &self,
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
        cancelled: Arc<AtomicBool>,
    ) -> BalPrewarmBlock<'_> {
        let mut started = true;
        for worker in &self.workers {
            started &= worker
                .send(PrewarmMsg::BeginBlock {
                    build: build.clone(),
                    caches: caches.clone(),
                    txpool_snapshot: txpool_snapshot.clone(),
                    cancelled: cancelled.clone(),
                })
                .is_ok();
        }
        BalPrewarmBlock { pool: self, cancelled, started, finished: false }
    }

    /// Fire-and-forget: warm an account (basic account + bytecode) on some worker.
    fn warm_account(&self, addr: Address) {
        self.send_warm(PrewarmTarget::Account(addr));
    }

    /// Fire-and-forget: warm one storage slot on some worker.
    fn warm_storage(&self, addr: Address, slot: StorageKey) {
        self.send_warm(PrewarmTarget::Storage(addr, slot));
    }

    /// Ends the block: every worker drops its provider (and read txn) once it has drained the warm
    /// requests queued ahead of this message.
    ///
    /// Blocks until all workers processed the end block message.
    fn end_block(&self) -> bool {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(SendOnDrop { sender: Some(tx) });

        let mut sent = true;
        for worker in &self.workers {
            sent &= worker.send(PrewarmMsg::EndBlock(tx.clone())).is_ok();
        }

        drop(tx);
        sent && rx.blocking_recv().is_ok()
    }

    fn send_warm(&self, target: PrewarmTarget) {
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let _ = self.workers[i].send(PrewarmMsg::Warm(target));
    }
}

/// A BAL prewarm generation that closes every worker's provider when dropped.
///
/// Keeping the end marker in a guard prevents panics or early returns in the dispatcher from
/// leaving read transactions pinned or mixing a later block with the current generation.
pub(crate) struct BalPrewarmBlock<'a> {
    pool: &'a BalPrewarmPool,
    cancelled: Arc<AtomicBool>,
    started: bool,
    finished: bool,
}

impl BalPrewarmBlock<'_> {
    /// Queues an account read in this generation.
    pub(crate) fn warm_account(&self, addr: Address) {
        self.pool.warm_account(addr);
    }

    /// Queues a storage read in this generation.
    pub(crate) fn warm_storage(&self, addr: Address, slot: StorageKey) {
        self.pool.warm_storage(addr, slot);
    }

    /// Ends this generation and returns whether every worker processed its end marker.
    pub(crate) fn finish(mut self) -> bool {
        self.finished = true;
        let ended = self.pool.end_block();
        self.started && ended
    }
}

impl Drop for BalPrewarmBlock<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.cancelled.store(true, Ordering::Relaxed);
            let _ = self.pool.end_block();
        }
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
/// However, that overhead is considered negligible compared to the benefits of fully utilizing
/// `NVMe` resources. For example, with request latency of 100µs, 100k IO requests the expected
/// time to finish is 312.5ms at QD=32 and 156.26ms at QD=64.
///
/// This should explain why this particular value is picked.
pub(crate) const DEFAULT_BAL_PREWARM_THREADS: usize = 128;

fn prewarm_loop(rx: crossbeam_channel::Receiver<PrewarmMsg>) {
    // The provider (and its MDBX read txn) held for the current block, between `BeginBlock` and
    // `EndBlock`. `None` while idle, so no read txn is pinned across the inter-block gap.
    let mut provider: Option<CachedStateProvider<StateProviderBox>> = None;
    let mut cancelled: Option<Arc<AtomicBool>> = None;

    // Blocks when idle; the channel disconnects (and the loop ends) when the pool is dropped.
    while let Ok(msg) = rx.recv() {
        match msg {
            PrewarmMsg::BeginBlock {
                build,
                caches,
                txpool_snapshot,
                cancelled: block_cancelled,
            } => {
                provider = if block_cancelled.load(Ordering::Relaxed) {
                    None
                } else {
                    match (build)() {
                        Ok(inner) => Some(
                            CachedStateProvider::new_prewarm(inner, caches)
                                .with_txpool_snapshot(txpool_snapshot),
                        ),
                        Err(err) => {
                            trace!(target: "engine::tree::bal_prewarm_pool", %err, "failed to build provider");
                            None
                        }
                    }
                };
                cancelled = Some(block_cancelled);
            }
            PrewarmMsg::Warm(target) => {
                if cancelled.as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    continue
                }
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
            PrewarmMsg::EndBlock(end_tx) => {
                provider = None;
                cancelled = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;
    use reth_execution_cache::CachedStatus;
    use reth_provider::test_utils::MockEthProvider;
    use std::{convert::Infallible, time::Duration};

    #[test]
    fn cancellation_discards_queued_warm_reads() {
        let pool = BalPrewarmPool::new(1);
        let caches = ExecutionCache::new(1_000);
        let cancelled = Arc::new(AtomicBool::new(false));
        let address = Address::repeat_byte(0x11);
        let (build_started_tx, build_started_rx) = crossbeam_channel::bounded(1);
        let (release_build_tx, release_build_rx) = crossbeam_channel::bounded(0);
        let provider = MockEthProvider::default();
        provider.enable_database_provider();
        let build = Arc::new(move || {
            let _ = build_started_tx.send(());
            let _ = release_build_rx.recv();
            Ok(Box::new(provider.clone()) as StateProviderBox)
        });

        let block = pool.begin_block(build, caches.clone(), None, Arc::clone(&cancelled));
        build_started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        block.warm_account(address);
        cancelled.store(true, Ordering::Relaxed);
        release_build_tx.send(()).unwrap();

        assert!(block.finish());
        let status =
            caches.get_or_try_insert_account_with(address, || Ok::<_, Infallible>(None)).unwrap();
        assert_eq!(status, CachedStatus::NotCached(None));
    }
}
