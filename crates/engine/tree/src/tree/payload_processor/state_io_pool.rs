use alloy_primitives::{Address, StorageKey, StorageValue, B256};
use reth_execution_cache::{CachedStateProvider, ExecutionCache};
use reth_primitives_traits::Account;
use reth_provider::{
    AccountReader, BytecodeReader, ProviderError, ProviderResult, StateProvider, StateProviderBox,
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
pub(crate) type BuildProviderFn = dyn Fn() -> ProviderResult<StateProviderBox> + Send + Sync;

/// Long-lived pool of blocking threads for parent-state I/O during payload processing.
#[derive(Debug)]
pub(crate) struct StateIoPool {
    /// One queue per worker. `BeginBlock`/barriers/`EndBlock` are broadcast to all; work is
    /// round-robined.
    workers: Vec<crossbeam_channel::Sender<StateIoMsg>>,
    /// Round-robin cursor for distributing warm and read requests across workers.
    next: AtomicUsize,
    _handles: Vec<JoinHandle<()>>,
}

impl StateIoPool {
    /// Spawns `num_threads` long-lived blocking worker threads. Owned by the
    /// [`PayloadProcessor`](super::PayloadProcessor); the threads exit when the pool is dropped.
    pub(crate) fn new(num_threads: usize) -> Arc<Self> {
        assert!(num_threads > 0, "state I/O pool requires at least one worker");

        let mut workers = Vec::with_capacity(num_threads);
        let mut handles = Vec::with_capacity(num_threads);
        for i in 0..num_threads {
            let (tx, rx) = crossbeam_channel::unbounded::<StateIoMsg>();
            workers.push(tx);
            handles.push(
                std::thread::Builder::new()
                    .name(format!("state-io-{i:03}"))
                    .spawn(move || state_io_loop(rx))
                    .expect("spawn state I/O thread"),
            );
        }
        trace!(target: "engine::tree::state_io_pool", num_threads, "StateIoPool spawned");
        Arc::new(Self { workers, next: AtomicUsize::new(0), _handles: handles })
    }

    /// Begins a block: hands every worker the provider builder and shared cache so each opens its
    /// own read txn over the parent state. Pair with [`end_block`](Self::end_block).
    pub(crate) fn begin_block(
        &self,
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        read_results: Option<crossbeam_channel::Sender<StateIoReadResult>>,
    ) {
        for worker in &self.workers {
            let _ = worker.send(StateIoMsg::BeginBlock {
                build: build.clone(),
                caches: caches.clone(),
                read_results: read_results.clone(),
            });
        }
    }

    /// Fire-and-forget: warm an account (basic account + bytecode) on some worker.
    pub(crate) fn warm_account(&self, address: Address) {
        self.send_work(StateIoMsg::Warm(StateIoWarmTarget::Account(address)));
    }

    /// Fire-and-forget: warm one storage slot on some worker.
    pub(crate) fn warm_storage(&self, address: Address, slot: StorageKey) {
        self.send_work(StateIoMsg::Warm(StateIoWarmTarget::Storage(address, slot)));
    }

    /// Reads an account from the parent state and sends the result through the block's result
    /// channel.
    #[allow(dead_code)]
    pub(crate) fn read_account(&self, address: Address, hashed_address: B256) {
        self.send_work(StateIoMsg::Read(StateIoReadTarget::Account { address, hashed_address }));
    }

    /// Reads one storage slot from the parent state and sends the result through the block's result
    /// channel.
    #[allow(dead_code)]
    pub(crate) fn read_storage(
        &self,
        address: Address,
        hashed_address: B256,
        slot: StorageKey,
        hashed_slot: B256,
    ) {
        self.send_work(StateIoMsg::Read(StateIoReadTarget::Storage {
            address,
            hashed_address,
            slot,
            hashed_slot,
        }));
    }

    /// Returns a barrier that completes after all work queued before it has been drained by every
    /// worker. Providers stay open.
    #[allow(dead_code)]
    pub(crate) fn barrier(&self) -> StateIoBarrier {
        let (ack_tx, ack_rx) = crossbeam_channel::unbounded();
        for worker in &self.workers {
            let _ = worker.send(StateIoMsg::Barrier { ack: ack_tx.clone() });
        }
        StateIoBarrier { rx: ack_rx, expected: self.workers.len() }
    }

    /// Ends the block: every worker drops its provider once it has drained requests queued ahead of
    /// the end marker. The returned barrier may be waited by callers that require completion.
    pub(crate) fn end_block(&self) -> StateIoBarrier {
        let (ack_tx, ack_rx) = crossbeam_channel::unbounded();
        for worker in &self.workers {
            let _ = worker.send(StateIoMsg::EndBlock { ack: ack_tx.clone() });
        }
        StateIoBarrier { rx: ack_rx, expected: self.workers.len() }
    }

    fn send_work(&self, msg: StateIoMsg) {
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let _ = self.workers[i].send(msg);
    }
}

/// Number of parent-state I/O threads.
///
/// The work performed on those threads boils down mostly to MDBX reads. An MDBX read consists of a
/// tree traversal and major page faults causing I/O.
///
/// To utilize NVMe parallelism we need enough in-flight work. Modern NVMe devices require roughly
/// 64-128 requests in-flight to reach peak throughput. If the OS page cache hit rate is high, this
/// can become CPU-bound, but the overhead is acceptable for the BAL and Lthash read sets this pool
/// serves.
pub(crate) const DEFAULT_STATE_IO_THREADS: usize = 128;

/// A read result emitted by [`StateIoPool`].
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum StateIoReadResult {
    Account { hashed_address: B256, account: ProviderResult<Option<Account>> },
    Storage { hashed_address: B256, hashed_slot: B256, value: ProviderResult<StorageValue> },
}

/// Waits for all workers to acknowledge a barrier or block end.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct StateIoBarrier {
    rx: crossbeam_channel::Receiver<()>,
    expected: usize,
}

impl StateIoBarrier {
    #[allow(dead_code)]
    pub(crate) fn wait(self) {
        for _ in 0..self.expected {
            if self.rx.recv().is_err() {
                break
            }
        }
    }
}

/// A single warm request: a whole account (basic account + its bytecode) or one storage slot.
enum StateIoWarmTarget {
    Account(Address),
    Storage(Address, StorageKey),
}

/// A read request. Plain keys are used for provider reads; hashed keys are returned to the caller.
#[allow(dead_code)]
enum StateIoReadTarget {
    Account { address: Address, hashed_address: B256 },
    Storage { address: Address, hashed_address: B256, slot: StorageKey, hashed_slot: B256 },
}

/// A message in a worker's queue. The per-block lifecycle is explicit and ordered.
#[allow(dead_code)]
enum StateIoMsg {
    /// Open a read txn for the new block: build a provider over the parent state and hold it.
    BeginBlock {
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        read_results: Option<crossbeam_channel::Sender<StateIoReadResult>>,
    },
    /// Warm one target into the held provider's cache. Ignored if no provider is held.
    Warm(StateIoWarmTarget),
    /// Read one target and send the result through the block result channel.
    Read(StateIoReadTarget),
    /// A FIFO marker used by callers that need to know all earlier work has completed.
    Barrier { ack: crossbeam_channel::Sender<()> },
    /// Drop the held provider and acknowledge once all earlier work has completed.
    EndBlock { ack: crossbeam_channel::Sender<()> },
}

enum ProviderState {
    Ready(CachedStateProvider<StateProviderBox>),
    Failed(ProviderError),
}

fn state_io_loop(rx: crossbeam_channel::Receiver<StateIoMsg>) {
    // The provider and its MDBX read txn are held only between BeginBlock and EndBlock. Keeping it
    // across idle time would pin old database pages.
    let mut provider: Option<ProviderState> = None;
    let mut read_results: Option<crossbeam_channel::Sender<StateIoReadResult>> = None;

    while let Ok(msg) = rx.recv() {
        match msg {
            StateIoMsg::BeginBlock { build, caches, read_results: results } => {
                read_results = results;
                provider = match (build)() {
                    Ok(inner) => {
                        Some(ProviderState::Ready(CachedStateProvider::new_prewarm(inner, caches)))
                    }
                    Err(err) => {
                        trace!(target: "engine::tree::state_io_pool", %err, "failed to build provider");
                        Some(ProviderState::Failed(err))
                    }
                };
            }
            StateIoMsg::Warm(target) => {
                let Some(ProviderState::Ready(provider)) = provider.as_ref() else { continue };
                warm_target(provider, target);
            }
            StateIoMsg::Read(target) => {
                let Some(read_results) = read_results.as_ref() else { continue };
                let _ = read_results.send(read_target(provider.as_ref(), target));
            }
            StateIoMsg::Barrier { ack } => {
                let _ = ack.send(());
            }
            StateIoMsg::EndBlock { ack } => {
                provider = None;
                read_results = None;
                let _ = ack.send(());
            }
        }
    }
}

fn warm_target(provider: &CachedStateProvider<StateProviderBox>, target: StateIoWarmTarget) {
    match target {
        StateIoWarmTarget::Account(address) => {
            if let Ok(Some(account)) = provider.basic_account(&address) &&
                let Some(code_hash) = account.bytecode_hash &&
                code_hash != alloy_consensus::constants::KECCAK_EMPTY
            {
                let _ = provider.bytecode_by_hash(&code_hash);
            }
        }
        StateIoWarmTarget::Storage(address, slot) => {
            let _ = provider.storage(address, slot);
        }
    }
}

fn read_target(provider: Option<&ProviderState>, target: StateIoReadTarget) -> StateIoReadResult {
    match target {
        StateIoReadTarget::Account { address, hashed_address } => {
            let account =
                provider_for_read(provider).and_then(|provider| provider.basic_account(&address));
            StateIoReadResult::Account { hashed_address, account }
        }
        StateIoReadTarget::Storage { address, hashed_address, slot, hashed_slot } => {
            let value = provider_for_read(provider).and_then(|provider| {
                provider.storage(address, slot).map(Option::unwrap_or_default)
            });
            StateIoReadResult::Storage { hashed_address, hashed_slot, value }
        }
    }
}

fn provider_for_read(
    provider: Option<&ProviderState>,
) -> ProviderResult<&CachedStateProvider<StateProviderBox>> {
    match provider {
        Some(ProviderState::Ready(provider)) => Ok(provider),
        Some(ProviderState::Failed(err)) => Err(err.clone()),
        None => Err(ProviderError::UnsupportedProvider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_result_is_sent_before_end_block_barrier_on_build_error() {
        let pool = StateIoPool::new(1);
        let (read_tx, read_rx) = crossbeam_channel::unbounded();
        let build: Arc<BuildProviderFn> = Arc::new(|| Err(ProviderError::UnsupportedProvider));

        pool.begin_block(build, ExecutionCache::new(1), Some(read_tx));
        pool.read_storage(Address::ZERO, B256::ZERO, StorageKey::ZERO, B256::ZERO);
        pool.end_block().wait();

        let result = read_rx.try_recv().expect("queued read result");
        match result {
            StateIoReadResult::Storage { hashed_address, hashed_slot, value } => {
                assert_eq!(hashed_address, B256::ZERO);
                assert_eq!(hashed_slot, B256::ZERO);
                assert!(matches!(value, Err(ProviderError::UnsupportedProvider)));
            }
            StateIoReadResult::Account { .. } => panic!("unexpected account result"),
        }
    }

    #[test]
    fn warm_requests_do_not_require_read_results() {
        let pool = StateIoPool::new(1);
        let build: Arc<BuildProviderFn> = Arc::new(|| Err(ProviderError::UnsupportedProvider));

        pool.begin_block(build, ExecutionCache::new(1), None);
        pool.warm_account(Address::ZERO);
        pool.warm_storage(Address::ZERO, StorageKey::ZERO);
        pool.end_block().wait();
    }
}
