//! BAL read-set prewarming pool.

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
pub type BuildProviderFn = dyn Fn() -> ProviderResult<StateProviderBox> + Send + Sync;

/// An account and a batch of its storage slots, or a batch of storage slots on their own.
enum PrewarmTarget {
    Account(Address, Box<[StorageKey]>),
    Storage(Address, Box<[StorageKey]>),
}

/// A message in a worker's queue. The per-block lifecycle is explicit and ordered (the queue is
/// FIFO): `BeginBlock`, the worker's share of `Warm`s, `FinishState`, then `EndBlock`.
enum PrewarmMsg {
    /// Open a read txn for the new block: build a provider over the parent state and hold it.
    BeginBlock {
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
        stop: Arc<AtomicBool>,
    },
    /// Warm one target into the held provider's cache. Ignored if no provider is held.
    Warm(PrewarmTarget),
    /// Signal that this worker has finished account and storage reads.
    FinishState(Arc<SendOnDrop>),
    /// Warm deferred bytecode, then drop the held provider (and its read txn).
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
        stop: Arc<AtomicBool>,
    ) {
        for worker in &self.workers {
            let _ = worker.send(PrewarmMsg::BeginBlock {
                build: build.clone(),
                caches: caches.clone(),
                txpool_snapshot: txpool_snapshot.clone(),
                stop: stop.clone(),
            });
        }
    }

    /// Fire-and-forget: warm an account and its storage slots, deferring bytecode until all
    /// account and storage requests have finished.
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

    /// Waits for all account and storage requests, then warms deferred bytecode and drops each
    /// worker's provider (and read txn). Workers skip remaining reads when execution stops.
    ///
    /// Blocks until all workers processed the end block message.
    pub fn end_block(&self) {
        // A queue-local boundary would let faster workers read code while slower workers still
        // fetch accounts and storage. Wait for every queue before starting any bytecode reads.
        self.synchronize(PrewarmMsg::FinishState);
        self.synchronize(PrewarmMsg::EndBlock);
    }

    fn synchronize(&self, message: impl Fn(Arc<SendOnDrop>) -> PrewarmMsg) {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(SendOnDrop { sender: Some(tx) });

        for worker in &self.workers {
            let _ = worker.send(message(tx.clone()));
        }

        drop(tx);
        rx.blocking_recv().expect("BAL prewarm pool dropped without signaling completion");
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
/// However, that overhead is considered negligible compared to the benefits of fully utilizing
/// `NVMe` resources. For example, with request latency of 100µs, 100k IO requests the expected
/// time to finish is 312.5ms at QD=32 and 156.26ms at QD=64.
///
/// This should explain why this particular value is picked.
pub const DEFAULT_BAL_PREWARM_THREADS: usize = 128;

/// Number of storage slots carried by one warm message.
///
/// Batching amortizes the send over many slots and hands the worker a run of slots that live
/// close together in the storage table, while the cap keeps enough messages in flight to saturate
/// the workers on blocks whose read-set is concentrated in a few accounts.
const WARM_BATCH_SIZE: usize = 8;

fn prewarm_loop(rx: crossbeam_channel::Receiver<PrewarmMsg>) {
    // The provider (and its MDBX read txn) held for the current block, between `BeginBlock` and
    // `EndBlock`. `None` while idle, so no read txn is pinned across the inter-block gap.
    let mut provider: Option<CachedStateProvider<StateProviderBox>> = None;
    let mut stop = Arc::new(AtomicBool::new(false));
    let mut code_hashes = Vec::new();

    // Blocks when idle; the channel disconnects (and the loop ends) when the pool is dropped.
    while let Ok(msg) = rx.recv() {
        match msg {
            PrewarmMsg::BeginBlock { build, caches, txpool_snapshot, stop: block_stop } => {
                stop = block_stop;
                code_hashes.clear();
                if stop.load(Ordering::Relaxed) {
                    provider = None;
                    continue
                }
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
            PrewarmMsg::Warm(target) => {
                if stop.load(Ordering::Relaxed) {
                    continue
                }
                let Some(provider) = provider.as_ref() else { continue };
                match target {
                    PrewarmTarget::Account(addr, slots) => {
                        if let Ok(Some(account)) = provider.basic_account(&addr) &&
                            let Some(code_hash) = account.bytecode_hash &&
                            code_hash != alloy_consensus::constants::KECCAK_EMPTY
                        {
                            code_hashes.push(code_hash);
                        }
                        for &slot in &slots {
                            if stop.load(Ordering::Relaxed) {
                                break
                            }
                            let _ = provider.storage(addr, slot);
                        }
                    }
                    PrewarmTarget::Storage(addr, slots) => {
                        for &slot in &slots {
                            if stop.load(Ordering::Relaxed) {
                                break
                            }
                            let _ = provider.storage(addr, slot);
                        }
                    }
                }
            }
            PrewarmMsg::FinishState(done) => drop(done),
            PrewarmMsg::EndBlock(end_tx) => {
                if let Some(provider) = &provider {
                    for code_hash in &code_hashes {
                        if stop.load(Ordering::Relaxed) {
                            break
                        }
                        // The shared cache also covers code loaded by execution in the meantime.
                        let _ = provider.bytecode_by_hash(code_hash);
                    }
                }
                code_hashes.clear();
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Bytes, B256, U256};
    use reth_execution_cache::CachedStatus;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use std::time::Duration;

    #[test]
    fn code_phase_waits_for_every_worker() {
        let (tx0, rx0) = crossbeam_channel::unbounded();
        let (tx1, rx1) = crossbeam_channel::unbounded();
        let pool =
            BalPrewarmPool { workers: vec![tx0, tx1], next: AtomicUsize::new(0), _handles: vec![] };
        let end = std::thread::spawn(move || pool.end_block());
        let timeout = Duration::from_secs(10);
        let PrewarmMsg::FinishState(done0) = rx0.recv_timeout(timeout).unwrap() else {
            panic!("expected state completion boundary")
        };
        let PrewarmMsg::FinishState(done1) = rx1.recv_timeout(timeout).unwrap() else {
            panic!("expected state completion boundary")
        };
        drop(done0);
        // A fast worker must not start code while another worker still reads state.
        assert!(matches!(
            rx0.recv_timeout(Duration::from_millis(50)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout)
        ));
        drop(done1);
        for rx in [&rx0, &rx1] {
            let PrewarmMsg::EndBlock(done) = rx.recv_timeout(timeout).unwrap() else {
                panic!("expected code phase and block completion")
            };
            drop(done);
        }
        end.join().unwrap();
    }

    #[test]
    fn warms_accounts_and_storage_before_code() {
        let pool = BalPrewarmPool::new(4);
        let caches = ExecutionCache::new(1024 * 1024);
        let provider = MockEthProvider::default();
        let addresses = [Address::with_last_byte(1), Address::with_last_byte(2)];
        let code = Bytes::from_static(&[0x60, 0x01, 0x00]);
        let code_hash = keccak256(&code);
        let slots: Vec<_> = (0..WARM_BATCH_SIZE * 3 + 1)
            .map(|i| (B256::with_last_byte(i as u8), U256::from(i + 1)))
            .collect();
        for address in addresses {
            provider.add_account(
                address,
                ExtendedAccount::new(1, U256::from(10))
                    .with_bytecode(code.clone())
                    .extend_storage(slots.iter().copied()),
            );
        }
        pool.begin_block(
            Arc::new(move || Ok(Box::new(provider.clone()))),
            caches.clone(),
            None,
            Arc::default(),
        );
        for address in addresses {
            pool.warm_account(address, slots.iter().map(|(key, _)| *key));
        }
        pool.synchronize(PrewarmMsg::FinishState);

        for address in addresses {
            assert!(matches!(
                caches.get_or_try_insert_account_with(address, || Err(())),
                Ok(CachedStatus::Cached(Some(_)))
            ));
            for &(key, value) in &slots {
                assert_eq!(
                    caches.get_or_try_insert_storage_with(address, key, || Err(())),
                    Ok(CachedStatus::Cached(value))
                );
            }
        }
        assert_eq!(caches.get_or_try_insert_code_with(code_hash, || Err(())), Err(()));

        pool.end_block();
        assert!(matches!(
            caches.get_or_try_insert_code_with(code_hash, || Err(())),
            Ok(CachedStatus::Cached(Some(bytecode))) if bytecode.original_bytes() == code
        ));
    }

    #[test]
    fn cancellation_skips_deferred_code_and_queued_state() {
        let pool = BalPrewarmPool::new(2);
        let provider = MockEthProvider::default();
        let address = Address::with_last_byte(1);
        let code = Bytes::from_static(&[0x60, 0x01, 0x00]);
        let code_hash = keccak256(&code);
        provider.add_account(address, ExtendedAccount::new(1, U256::ZERO).with_bytecode(code));
        let build: Arc<BuildProviderFn> = Arc::new(move || Ok(Box::new(provider.clone())));

        for cancel in [true, false] {
            let caches = ExecutionCache::new(1024 * 1024);
            let stop = Arc::new(AtomicBool::new(false));
            pool.begin_block(build.clone(), caches.clone(), None, stop.clone());
            pool.warm_account(address, []);
            pool.synchronize(PrewarmMsg::FinishState);
            stop.store(cancel, Ordering::Relaxed);

            let queued_address = Address::with_last_byte(2);
            pool.warm_account(queued_address, [B256::ZERO]);
            pool.end_block();

            let account = caches.get_or_try_insert_account_with(queued_address, || Err(()));
            let storage =
                caches.get_or_try_insert_storage_with(queued_address, B256::ZERO, || Err(()));
            let code = caches.get_or_try_insert_code_with(code_hash, || Err(()));
            if cancel {
                assert_eq!(account, Err(()));
                assert_eq!(storage, Err(()));
                assert_eq!(code, Err(()));
            } else {
                assert_eq!(account, Ok(CachedStatus::Cached(None)));
                assert_eq!(storage, Ok(CachedStatus::Cached(U256::ZERO)));
                assert!(matches!(code, Ok(CachedStatus::Cached(Some(_)))));
            }
        }
    }
}
