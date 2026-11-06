//! BAL read-set prewarming pool.

use alloy_primitives::{Address, StorageKey, B256};
use reth_execution_cache::{CachedStateProvider, ExecutionCache, TxPoolPrewarmCacheSnapshot};
use reth_provider::{EvmStateProvider, EvmStateProviderBox, ProviderResult};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::JoinHandle,
};
use tokio::sync::oneshot;
use tracing::trace;

/// Builds a fresh EVM provider over the block's parent state. Type-erased so the pool is not
/// generic over the provider factory; each worker builds its own per block.
pub type BuildProviderFn = dyn Fn() -> ProviderResult<EvmStateProviderBox> + Send + Sync;

/// A single warm request: an account and a batch of its storage slots, or a batch of storage slots
/// on their own.
enum PrewarmTarget {
    Account(Address, Box<[StorageKey]>),
    Storage(Address, Box<[StorageKey]>),
    Code(B256),
}

/// A message in a worker's queue. The per-block lifecycle is explicit and ordered (the queue is
/// FIFO): one `BeginBlock`, then the worker's share of `Warm`s, then one `EndBlock`.
enum PrewarmMsg {
    /// Open a read txn for the new block: build a provider over the parent state and hold it.
    BeginBlock {
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
    },
    /// Warm one target into the held provider's cache, then count it done on the latch, if any.
    /// Ignored if no provider is held.
    Warm(PrewarmTarget, Option<Arc<WarmLatch>>),
    /// Drop the held provider (and its read txn).
    EndBlock(Arc<SendOnDrop>),
}

/// Long-lived pool of blocking threads that warm the BAL read-set into the shared execution cache.
#[derive(Debug)]
pub struct BalPrewarmPool {
    /// One queue per worker. `BeginBlock`/`EndBlock` are broadcast to all; `Warm`s round-robin.
    workers: Vec<crossbeam_channel::Sender<PrewarmMsg>>,
    /// Round-robin cursor for distributing warm requests across workers.
    next: AtomicUsize,
    _handles: Vec<JoinHandle<()>>,
}

impl BalPrewarmPool {
    /// Spawns `num_threads` long-lived blocking worker threads. Owned by the
    /// [`PayloadProcessor`](super::PayloadProcessor); the threads exit when the pool is dropped.
    pub fn new(num_threads: usize) -> Arc<Self> {
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
    pub fn begin_block(
        &self,
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
    ) {
        for worker in &self.workers {
            let _ = worker.send(PrewarmMsg::BeginBlock {
                build: build.clone(),
                caches: caches.clone(),
                txpool_snapshot: txpool_snapshot.clone(),
            });
        }
    }

    /// Fire-and-forget: warm an account and its storage slots.
    ///
    /// The slots are dispatched in `WARM_BATCH_SIZE` chunks that are distributed independently,
    /// so a single account with a large read-set does not serialize onto one worker;
    /// [`end_block`](Self::end_block) waits for the slowest queue.
    pub fn warm_account(&self, addr: Address, slots: impl IntoIterator<Item = StorageKey>) {
        let mut slots = slots.into_iter();
        let mut batch: Box<[StorageKey]> = slots.by_ref().take(WARM_BATCH_SIZE).collect();
        self.send_warm(PrewarmTarget::Account(addr, batch));

        loop {
            batch = slots.by_ref().take(WARM_BATCH_SIZE).collect();
            if batch.is_empty() {
                break
            }
            self.send_warm(PrewarmTarget::Storage(addr, batch));
        }
    }

    /// Fire-and-forget: warm storage slots of an account without loading the account.
    pub fn warm_storage(&self, addr: Address, slots: impl IntoIterator<Item = StorageKey>) {
        let mut slots = slots.into_iter();
        loop {
            let batch: Box<[StorageKey]> = slots.by_ref().take(WARM_BATCH_SIZE).collect();
            if batch.is_empty() {
                break
            }
            self.send_warm(PrewarmTarget::Storage(addr, batch));
        }
    }

    /// Fire-and-forget: warm the bytecode with `hash`.
    pub fn warm_code(&self, hash: B256) {
        self.send_warm(PrewarmTarget::Code(hash));
    }

    /// Warms `accounts`, `storage` slots and `code` in parallel across a share of the workers and
    /// returns a latch that [`WarmLatch::wait`] blocks on until all of them are loaded.
    ///
    /// Only one in [`WARM_ALL_SHARE`] workers takes these loads, which bounds how many of them
    /// queue on the disk ahead of block execution's own reads of a chain of dependent keys.
    pub fn warm_all(
        &self,
        accounts: impl IntoIterator<Item = Address>,
        storage: &[(Address, StorageKey)],
        code: impl IntoIterator<Item = B256>,
    ) -> Arc<WarmLatch> {
        let latch = Arc::new(WarmLatch::default());
        let send = |target| {
            latch.remaining.fetch_add(1, Ordering::AcqRel);
            let workers = (self.workers.len() / WARM_ALL_SHARE).max(1);
            let i = self.next.fetch_add(1, Ordering::Relaxed) % workers;
            if self.workers[i].send(PrewarmMsg::Warm(target, Some(Arc::clone(&latch)))).is_err() {
                latch.done();
            }
        };
        for address in accounts {
            send(PrewarmTarget::Account(address, Box::default()));
        }
        for chunk in storage.chunk_by(|a, b| a.0 == b.0) {
            for batch in chunk.chunks(WARM_BATCH_SIZE) {
                send(PrewarmTarget::Storage(
                    batch[0].0,
                    batch.iter().map(|(_, slot)| *slot).collect(),
                ));
            }
        }
        for hash in code {
            send(PrewarmTarget::Code(hash));
        }
        latch
    }

    /// Ends the block: every worker drops its provider (and read txn) once it has drained the warm
    /// requests queued ahead of this message.
    ///
    /// Blocks until all workers processed the end block message.
    pub fn end_block(&self) {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(SendOnDrop { sender: Some(tx) });

        for worker in &self.workers {
            let _ = worker.send(PrewarmMsg::EndBlock(tx.clone()));
        }

        drop(tx);
        rx.blocking_recv().expect("BAL prewarm pool dropped without signaling completion");
    }

    fn send_warm(&self, target: PrewarmTarget) {
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let _ = self.workers[i].send(PrewarmMsg::Warm(target, None));
    }
}

/// Counts outstanding warm requests of one [`BalPrewarmPool::warm_all`] call.
#[derive(Debug, Default)]
pub struct WarmLatch {
    remaining: AtomicUsize,
    lock: Mutex<()>,
    loaded: Condvar,
}

impl WarmLatch {
    /// Blocks until every request of the call is loaded.
    pub fn wait(&self) {
        let mut guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        while self.remaining.load(Ordering::Acquire) > 0 {
            guard = self.loaded.wait(guard).unwrap_or_else(|err| err.into_inner());
        }
    }

    fn done(&self) {
        if self.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
            self.loaded.notify_all();
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
pub const DEFAULT_BAL_PREWARM_THREADS: usize = 128;

/// One in this many workers takes the loads of [`BalPrewarmPool::warm_all`].
///
/// On derek, which reads blocks from a cold page cache, loading probed keys on all 128 workers
/// slowed blocks whose heaviest transaction walks a chain of dependent reads; a quarter of them cut
/// p99 by 10%.
const WARM_ALL_SHARE: usize = 4;

/// Number of storage slots carried by one warm message.
///
/// Batching amortizes the send over many slots and hands the worker a run of slots that live
/// close together in the storage table, while the cap keeps enough messages in flight to saturate
/// the workers on blocks whose read-set is concentrated in a few accounts.
const WARM_BATCH_SIZE: usize = 8;

fn prewarm_loop(rx: crossbeam_channel::Receiver<PrewarmMsg>) {
    // Warming only loads ahead of execution, which must not wait for it.
    reth_tasks::utils::lower_background_worker_priority();
    // The provider (and its MDBX read txn) held for the current block, between `BeginBlock` and
    // `EndBlock`. `None` while idle, so no read txn is pinned across the inter-block gap.
    let mut provider: Option<CachedStateProvider<EvmStateProviderBox>> = None;

    // Blocks when idle; the channel disconnects (and the loop ends) when the pool is dropped.
    while let Ok(msg) = rx.recv() {
        match msg {
            PrewarmMsg::BeginBlock { build, caches, txpool_snapshot } => {
                provider = match (build)() {
                    Ok(inner) => Some(
                        CachedStateProvider::new_prewarm(inner, caches)
                            .with_txpool_snapshot(txpool_snapshot),
                    ),
                    Err(err) => {
                        trace!(target: "engine::tree::bal_prewarm_pool", %err, "failed to build provider");
                        None
                    }
                };
            }
            PrewarmMsg::Warm(target, latch) => {
                let _latch = LatchGuard(latch);
                let Some(provider) = provider.as_ref() else { continue };
                match target {
                    PrewarmTarget::Account(addr, slots) => {
                        let _ = provider.basic_account(&addr);
                        for &slot in &slots {
                            let _ = provider.storage(addr, slot);
                        }
                    }
                    PrewarmTarget::Storage(addr, slots) => {
                        for &slot in &slots {
                            let _ = provider.storage(addr, slot);
                        }
                    }
                    PrewarmTarget::Code(hash) => {
                        let _ = provider.bytecode_by_hash(&hash);
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

/// Counts a warm request done when dropped, whether or not it was loaded.
struct LatchGuard(Option<Arc<WarmLatch>>);

impl Drop for LatchGuard {
    fn drop(&mut self) {
        if let Some(latch) = &self.0 {
            latch.done();
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
    use alloy_primitives::U256;

    #[test]
    fn warm_latch_waits_for_every_request() {
        let latch = Arc::new(WarmLatch::default());
        latch.remaining.store(3, Ordering::Release);
        let workers = (0..3)
            .map(|_| {
                let latch = Arc::clone(&latch);
                std::thread::spawn(move || drop(LatchGuard(Some(latch))))
            })
            .collect::<Vec<_>>();
        latch.wait();
        assert_eq!(latch.remaining.load(Ordering::Acquire), 0);
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn warm_all_returns_once_requests_are_dropped_without_a_provider() {
        let pool = BalPrewarmPool::new(4);
        let slots = (0..20u64)
            .map(|i| (Address::ZERO, StorageKey::from(U256::from(i))))
            .collect::<Vec<_>>();
        // No block was begun, so the workers drop every request; the latch still completes.
        pool.warm_all([Address::ZERO], &slots, [B256::ZERO]).wait();
    }
}
