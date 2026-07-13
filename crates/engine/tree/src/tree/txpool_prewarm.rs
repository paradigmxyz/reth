//! Txpool-driven state prewarming and immutable snapshot publication.

use crate::tree::{
    StateProviderBuilder, StateProviderDatabase, TxPoolPrewarmCache, TxPoolPrewarmCacheSnapshot,
};
use alloy_consensus::{transaction::Recovered, Transaction as _};
use alloy_evm::Evm;
use alloy_primitives::{
    map::{AddressMap, B256Map, HashSet},
    Address, BlockNumber, StorageKey, StorageValue, B256,
};
use reth_engine_primitives::TxPoolPrewarmingConfig;
use reth_evm::{ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::{NodePrimitives, TxTy};
use reth_provider::{BlockReader, StateProviderBox, StateProviderFactory, StateReader};
use reth_revm::{database::EvmStateProvider, db::State};
use reth_trie_common::MultiProofTargetsV2;
use reth_trie_parallel::state_root_task::StateAccessHint;
use revm::DatabaseCommit;
use std::{
    fmt::Debug,
    marker::PhantomData,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
    time::{Duration, Instant},
};
use tracing::{debug, trace, warn};

/// Delay between txpool refreshes after a completed or empty selection.
const TXPOOL_REFRESH_INTERVAL: Duration = Duration::from_millis(100);

/// Delay while waiting for pool maintenance to advance to the state being warmed.
const TXPOOL_HEAD_POLL_INTERVAL: Duration = Duration::from_millis(10);

type TxPoolPrewarmMarker<N, P, Evm> = PhantomData<fn() -> (N, P, Evm)>;

/// A transaction selected from the txpool for cache-only prewarming.
#[derive(Debug, Clone)]
pub struct TxPoolPrewarmTransaction<N: NodePrimitives> {
    /// Transaction hash.
    pub hash: B256,
    /// Recovered sender.
    pub sender: Address,
    /// Recovered consensus transaction.
    pub transaction: Recovered<TxTy<N>>,
}

/// Transactions selected for one refresh of the reusable txpool cache.
#[derive(Debug, Clone)]
pub struct TxPoolPrewarmSelection<N: NodePrimitives> {
    /// Fresh selected transactions in block-builder order.
    pub transactions: Vec<TxPoolPrewarmTransaction<N>>,
    /// Fresh candidates scanned while selecting.
    pub scanned: usize,
    /// Sum of selected transaction gas limits.
    pub selected_gas: u64,
    /// Whether selection observed cancellation.
    pub canceled: bool,
}

impl<N: NodePrimitives> Default for TxPoolPrewarmSelection<N> {
    fn default() -> Self {
        Self { transactions: Vec::new(), scanned: 0, selected_gas: 0, canceled: false }
    }
}

impl<N: NodePrimitives> TxPoolPrewarmSelection<N> {
    /// Returns whether no transactions were selected.
    pub const fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// Returns the selected transaction count.
    pub const fn len(&self) -> usize {
        self.transactions.len()
    }
}

/// A live, forward-only view of the pool's best transactions for one canonical parent.
///
/// Returning [`None`](Iterator::next) only means no transaction is currently ready. The same
/// iterator can yield transactions that become pending later.
pub type TxPoolPrewarmTransactions<N> =
    Box<dyn Iterator<Item = TxPoolPrewarmTransaction<N>> + Send>;

/// Source of txpool transactions for best-effort cache prewarming.
pub trait TxPoolPrewarmSource<N: NodePrimitives>: Send + Sync + Debug {
    /// Returns the canonical block hash currently tracked by the pool.
    fn tracked_block_hash(&self) -> B256;

    /// Opens a live best-transactions iterator for `parent_hash`.
    ///
    /// The worker opens this once per canonical parent and retains it across empty polls, snapshot
    /// publications, and validation pauses. Sources should return [`None`] if they are not yet
    /// tracking `parent_hash`.
    fn best_transactions(&self, parent_hash: B256) -> Option<TxPoolPrewarmTransactions<N>>;
}

/// Immutable per-transaction state-root prefetch hints for one canonical parent.
#[derive(Clone, Debug, Default)]
pub struct TxPoolPrewarmHints {
    inner: Arc<B256Map<Arc<StateAccessHint>>>,
}

impl TxPoolPrewarmHints {
    pub(crate) fn new(hints: B256Map<Arc<StateAccessHint>>) -> Self {
        Self { inner: Arc::new(hints) }
    }

    fn snapshot(hints: &B256Map<Arc<StateAccessHint>>) -> Self {
        Self::new(hints.clone())
    }

    /// Returns the predicted state-root access hint for `transaction_hash`.
    pub fn get(&self, transaction_hash: &B256) -> Option<&StateAccessHint> {
        self.inner.get(transaction_hash).map(AsRef::as_ref)
    }

    /// Returns the number of transactions with published hints.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns whether no transaction hints were published.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// A request to warm txpool transactions against one fully validated parent state.
struct TxPoolPrewarmJob<N: NodePrimitives, P, Evm: ConfigureEvm<Primitives = N>> {
    parent_hash: B256,
    evm_env: EvmEnvFor<Evm>,
    provider_builder: StateProviderBuilder<N, P>,
    block_gas_limit: u64,
}

/// Cache data and trie hints published atomically for one canonical parent.
#[derive(Clone, Debug)]
pub(crate) struct TxPoolPrewarmSnapshot {
    cache: TxPoolPrewarmCacheSnapshot,
    hints: TxPoolPrewarmHints,
}

impl TxPoolPrewarmSnapshot {
    fn parent_hash(&self) -> B256 {
        self.cache.parent_hash()
    }

    pub(crate) fn into_parts(self) -> (TxPoolPrewarmCacheSnapshot, TxPoolPrewarmHints) {
        (self.cache, self.hints)
    }
}

/// Latest-wins canonical-head request slot shared with the worker.
struct TxPoolPrewarmRequests<J> {
    latest: Mutex<Option<J>>,
    available: Condvar,
    closed: AtomicBool,
}

impl<J> TxPoolPrewarmRequests<J> {
    const fn new() -> Self {
        Self { latest: Mutex::new(None), available: Condvar::new(), closed: AtomicBool::new(false) }
    }

    fn replace(&self, request: J) {
        *self.latest.lock().expect("txpool prewarm request lock poisoned") = Some(request);
        self.available.notify_one();
    }

    fn take(&self) -> Option<J> {
        self.latest.lock().expect("txpool prewarm request lock poisoned").take()
    }

    fn wait(&self, timeout: Option<Duration>) -> Option<J> {
        let mut latest = self.latest.lock().expect("txpool prewarm request lock poisoned");
        while latest.is_none() && !self.closed.load(Ordering::Acquire) {
            if let Some(timeout) = timeout {
                let (guard, result) = self
                    .available
                    .wait_timeout(latest, timeout)
                    .expect("txpool prewarm request lock poisoned");
                latest = guard;
                if result.timed_out() {
                    break
                }
            } else {
                latest = self.available.wait(latest).expect("txpool prewarm request lock poisoned");
            }
        }
        latest.take()
    }

    fn close(&self) {
        let _latest = self.latest.lock().expect("txpool prewarm request lock poisoned");
        self.closed.store(true, Ordering::Release);
        self.available.notify_all();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// Linearization point shared by worker publication and payload snapshot acquisition.
#[derive(Debug, Default)]
struct TxPoolSnapshotPublication {
    generation: AtomicU64,
    paused: AtomicBool,
    snapshot: RwLock<Option<TxPoolPrewarmSnapshot>>,
}

impl TxPoolSnapshotPublication {
    fn begin_head(&self) -> Option<u64> {
        let mut snapshot =
            self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        if self.paused.load(Ordering::Acquire) {
            return None
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *snapshot = None;
        Some(generation)
    }

    fn resume_head(&self) -> Option<u64> {
        let _snapshot = self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        if self.paused.load(Ordering::Acquire) {
            return None
        }
        Some(self.generation.fetch_add(1, Ordering::SeqCst) + 1)
    }

    fn cancel(&self) -> u64 {
        let _snapshot = self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        self.paused.store(true, Ordering::Release);
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn cancel_and_snapshot(&self, parent_hash: B256) -> (Option<TxPoolPrewarmSnapshot>, u64) {
        let snapshot = self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        self.paused.store(true, Ordering::Release);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        (
            snapshot.as_ref().filter(|snapshot| snapshot.parent_hash() == parent_hash).cloned(),
            generation,
        )
    }

    fn publish(&self, generation: u64, snapshot: TxPoolPrewarmSnapshot) -> bool {
        let mut published =
            self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        if self.generation.load(Ordering::Acquire) != generation {
            return false
        }
        *published = Some(snapshot);
        true
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) == generation
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    fn resume(&self, generation: u64) {
        let _snapshot = self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        if self.generation.load(Ordering::Acquire) == generation {
            self.paused.store(false, Ordering::Release);
        }
    }
}

/// Resumes head tracking after payload validation, including every early-return path.
pub(crate) struct TxPoolPrewarmPauseGuard {
    publication: Arc<TxPoolSnapshotPublication>,
    generation: u64,
}

#[derive(Debug, Default)]
struct TxPoolWorkerActivity {
    active: Mutex<bool>,
    idle: Condvar,
}

impl TxPoolWorkerActivity {
    fn try_enter(
        self: &Arc<Self>,
        publication: &TxPoolSnapshotPublication,
        generation: u64,
    ) -> Option<TxPoolWorkerActivityGuard> {
        let mut active = self.active.lock().expect("txpool worker activity lock poisoned");
        if *active || publication.is_paused() || !publication.is_current(generation) {
            return None
        }
        *active = true;
        Some(TxPoolWorkerActivityGuard { activity: Arc::clone(self) })
    }

    fn wait_until_idle(&self) -> Duration {
        let start = Instant::now();
        let mut active = self.active.lock().expect("txpool worker activity lock poisoned");
        while *active {
            active = self.idle.wait(active).expect("txpool worker activity lock poisoned");
        }
        start.elapsed()
    }
}

struct TxPoolWorkerActivityGuard {
    activity: Arc<TxPoolWorkerActivity>,
}

impl Drop for TxPoolWorkerActivityGuard {
    fn drop(&mut self) {
        *self.activity.active.lock().expect("txpool worker activity lock poisoned") = false;
        self.activity.idle.notify_all();
    }
}

impl Drop for TxPoolPrewarmPauseGuard {
    fn drop(&mut self) {
        self.publication.resume(self.generation);
    }
}

/// Coordinates a long-lived worker and the latest completed immutable snapshot.
pub(crate) struct TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    requests: Arc<TxPoolPrewarmRequests<TxPoolPrewarmJob<N, P, Evm>>>,
    publication: Arc<TxPoolSnapshotPublication>,
    activity: Arc<TxPoolWorkerActivity>,
    wait_generation: AtomicU64,
    _marker: TxPoolPrewarmMarker<N, P, Evm>,
}

impl<N, P, Evm> Debug for TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxPoolPrewarmHandle")
            .field("generation", &self.publication.generation.load(Ordering::Relaxed))
            .field(
                "published",
                &self
                    .publication
                    .snapshot
                    .read()
                    .expect("txpool snapshot publication lock poisoned")
                    .as_ref()
                    .map(|s| s.parent_hash()),
            )
            .finish_non_exhaustive()
    }
}

impl<N, P, Evm> TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Cancels speculative work and waits for any provider-backed EVM call to release its state.
    pub(crate) fn cancel_and_wait(&self) -> Duration {
        let generation = self.publication.cancel();
        self.wait_generation.store(generation, Ordering::Release);
        self.activity.wait_until_idle()
    }

    /// Resumes work after an explicit cache wait, including early-return payload paths.
    pub(crate) fn resume_after_wait(&self) {
        let generation = self.wait_generation.swap(0, Ordering::AcqRel);
        if generation > 0 {
            self.publication.resume(generation);
        }
    }
}

impl<N, P, Evm> TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Spawns the long-lived worker. Its mutable cache remains owned by this worker and is cleared
    /// between heads without releasing map capacity.
    pub(crate) fn spawn(
        runtime: &reth_tasks::Runtime,
        source: Arc<dyn TxPoolPrewarmSource<N>>,
        evm_config: Evm,
        config: TxPoolPrewarmingConfig,
    ) -> Self {
        let requests = Arc::new(TxPoolPrewarmRequests::new());
        let worker_requests = Arc::clone(&requests);
        let publication = Arc::new(TxPoolSnapshotPublication::default());
        let worker_publication = Arc::clone(&publication);
        let activity = Arc::new(TxPoolWorkerActivity::default());
        let worker_activity = Arc::clone(&activity);

        runtime.spawn_blocking_named("txpool-prewarm", move || {
            txpool_prewarm_loop(
                worker_requests,
                source,
                evm_config,
                config,
                worker_publication,
                worker_activity,
            )
        });

        Self {
            requests,
            publication,
            activity,
            wait_generation: AtomicU64::new(0),
            _marker: PhantomData,
        }
    }

    /// Cancels the active wave and returns the latest fully published snapshot for `parent_hash`.
    ///
    /// Taking the publication write lock linearizes cancellation with publication. The worker
    /// either published a completed snapshot before this method and it is returned, or observes the
    /// new generation and discards its unfinished wave.
    pub(crate) fn pause_and_snapshot(
        &self,
        parent_hash: B256,
    ) -> (Option<TxPoolPrewarmSnapshot>, TxPoolPrewarmPauseGuard) {
        let (snapshot, generation) = self.publication.cancel_and_snapshot(parent_hash);
        (
            snapshot,
            TxPoolPrewarmPauseGuard { publication: Arc::clone(&self.publication), generation },
        )
    }

    /// Starts continuous warming for the latest canonical head.
    pub(crate) fn start(
        &self,
        parent_hash: B256,
        evm_env: EvmEnvFor<Evm>,
        provider_builder: StateProviderBuilder<N, P>,
        block_gas_limit: u64,
    ) {
        self.requests.replace(TxPoolPrewarmJob {
            parent_hash,
            evm_env,
            provider_builder,
            block_gas_limit,
        });
    }
}

impl<N, P, Evm> Drop for TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    fn drop(&mut self) {
        self.publication.cancel();
        self.requests.close();
    }
}

fn txpool_prewarm_loop<N, P, Evm>(
    requests: Arc<TxPoolPrewarmRequests<TxPoolPrewarmJob<N, P, Evm>>>,
    source: Arc<dyn TxPoolPrewarmSource<N>>,
    evm_config: Evm,
    config: TxPoolPrewarmingConfig,
    publication: Arc<TxPoolSnapshotPublication>,
    activity: Arc<TxPoolWorkerActivity>,
) where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    let cache = TxPoolPrewarmCache::new();
    let mut hints = B256Map::<Arc<StateAccessHint>>::default();
    let mut warmed = HashSet::default();
    let mut warmed_per_sender = AddressMap::<usize>::default();
    let mut sender_prefixes = AddressMap::<Vec<TxPoolPrewarmTransaction<N>>>::default();
    let mut best_txs: Option<TxPoolPrewarmTransactions<N>> = None;
    let mut best_txs_parent_hash = None;
    let mut best_txs_failed = false;
    let mut deferred = None;
    let mut inflight: Option<TxPoolPrewarmSelection<N>> = None;
    let mut pending = None;
    let mut cache_parent_hash = None;

    while !requests.is_closed() {
        if let Some(request) = requests.take() {
            pending = Some(request);
        }
        if pending.is_none() {
            pending = requests.wait(None);
            continue
        }
        if publication.is_paused() {
            if let Some(request) = requests.wait(Some(TXPOOL_HEAD_POLL_INTERVAL)) {
                pending = Some(request);
            }
            continue
        }

        let tracked_hash = source.tracked_block_hash();
        if pending.as_ref().is_none_or(|job| job.parent_hash != tracked_hash) {
            if let Some(request) = requests.wait(Some(TXPOOL_HEAD_POLL_INTERVAL)) {
                pending = Some(request);
            }
            continue
        }
        let job = pending.take().expect("matching canonical request exists");

        if best_txs_parent_hash != Some(job.parent_hash) {
            let opened =
                catch_unwind(AssertUnwindSafe(|| source.best_transactions(job.parent_hash)));
            let opened = match opened {
                Ok(Some(opened)) => opened,
                Ok(None) => {
                    pending = Some(job);
                    std::thread::sleep(TXPOOL_HEAD_POLL_INTERVAL);
                    continue
                }
                Err(_) => {
                    warn!(
                        target: "engine::tree::txpool_prewarm",
                        parent_hash = ?job.parent_hash,
                        "opening txpool best-transactions iterator panicked"
                    );
                    pending = Some(job);
                    std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                    continue
                }
            };
            best_txs = Some(opened);
            best_txs_parent_hash = Some(job.parent_hash);
            best_txs_failed = false;
            deferred = None;
            inflight = None;
        }

        if source.tracked_block_hash() != job.parent_hash {
            pending = Some(job);
            continue
        }

        // Wait for pool maintenance to catch up to the authoritative canonical-head request
        // before invalidating the previous publication.
        let new_parent = cache_parent_hash != Some(job.parent_hash);
        let active_generation =
            if new_parent { publication.begin_head() } else { publication.resume_head() };
        let Some(active_generation) = active_generation else {
            if pending.is_none() {
                pending = Some(job);
            }
            continue
        };
        if new_parent {
            cache.clear();
            hints.clear();
            warmed.clear();
            warmed_per_sender.clear();
            sender_prefixes.clear();
            cache_parent_hash = Some(job.parent_hash);
        }
        debug!(
            target: "engine::tree::txpool_prewarm",
            parent_hash = ?job.parent_hash,
            "started txpool prewarming"
        );

        while publication.is_current(active_generation) && !publication.is_paused() {
            if let Some(request) = requests.take() {
                pending = Some(request);
            }
            if requests.is_closed() || publication.is_paused() {
                break
            }

            if source.tracked_block_hash() != job.parent_hash {
                break
            }

            if inflight.is_none() {
                if best_txs_failed {
                    std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                    continue
                }
                let (selection, cursor_panicked, cursor_exhausted) = select_best_transactions(
                    best_txs.as_mut().expect("best transactions opened for active parent"),
                    &mut deferred,
                    job.block_gas_limit,
                    config,
                    &warmed,
                    &warmed_per_sender,
                    &publication.paused,
                );
                if cursor_panicked {
                    best_txs_failed = true;
                    warn!(
                        target: "engine::tree::txpool_prewarm",
                        parent_hash = ?job.parent_hash,
                        "txpool best-transactions iterator panicked; retiring it for this parent"
                    );
                }
                if cursor_exhausted {
                    match catch_unwind(AssertUnwindSafe(|| {
                        source.best_transactions(job.parent_hash)
                    })) {
                        Ok(Some(opened)) => best_txs = Some(opened),
                        Ok(None) => {}
                        Err(_) => {
                            best_txs_failed = true;
                            warn!(
                                target: "engine::tree::txpool_prewarm",
                                parent_hash = ?job.parent_hash,
                                "reopening txpool best-transactions iterator panicked; retiring it for this parent"
                            );
                        }
                    }
                }
                let canceled = selection.canceled;
                if !selection.is_empty() {
                    inflight = Some(selection);
                }
                if canceled || !publication.is_current(active_generation) || publication.is_paused()
                {
                    break
                }
                if inflight.is_none() {
                    std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                    continue
                }
            }

            let sender_transactions = transactions_with_prefixes(
                &sender_prefixes,
                &inflight.as_ref().expect("non-empty selection is in flight").transactions,
            );
            let Some(activity_guard) = activity.try_enter(&publication, active_generation) else {
                break
            };
            let wave_hints = catch_unwind(AssertUnwindSafe(|| {
                prewarm_transactions(
                    &evm_config,
                    &job,
                    &cache,
                    &sender_transactions,
                    active_generation,
                    &publication,
                )
            }))
            .unwrap_or_else(|_| {
                warn!(
                    target: "engine::tree::txpool_prewarm",
                    parent_hash = ?job.parent_hash,
                    "txpool prewarming batch panicked"
                );
                None
            });
            drop(activity_guard);

            let Some(wave_hints) = wave_hints else {
                if publication.is_current(active_generation) && !publication.is_paused() {
                    std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                }
                break
            };
            if !publication.is_current(active_generation) {
                break
            }

            // Replaying a sender prefix can change later transactions' predicted state, and a
            // replacement can remove the old transaction hash entirely. Replace every hint for
            // affected prefixes as one completed wave instead of retaining stale entries.
            for sender in sender_transactions.keys() {
                if let Some(previous) = sender_prefixes.get(sender) {
                    for transaction in previous {
                        hints.remove(&transaction.hash);
                    }
                }
            }
            for transaction in sender_transactions.values().flatten() {
                hints.remove(&transaction.hash);
            }
            // The live iterator can keep producing transactions for a long-lived head. Reuse the
            // configured scan breadth as a hard bound on retained per-transaction hint state.
            for (transaction_hash, hint) in wave_hints {
                if hints.len() >= config.max_candidate_scan {
                    break
                }
                hints.insert(transaction_hash, hint);
            }

            // The deep clone happens privately. Only the short pointer swap below is serialized
            // with `newPayload` snapshot acquisition.
            let snapshot = TxPoolPrewarmSnapshot {
                cache: cache.snapshot(job.parent_hash),
                hints: TxPoolPrewarmHints::snapshot(&hints),
            };
            let (accounts, storage, bytecodes) = snapshot.cache.entry_counts();
            let hinted_transactions = snapshot.hints.len();
            if publication.publish(active_generation, snapshot) {
                let selection = inflight.take().expect("published selection is in flight");
                for transaction in &selection.transactions {
                    warmed.insert(transaction.hash);
                    *warmed_per_sender.entry(transaction.sender).or_default() += 1;
                }
                for (sender, transactions) in sender_transactions {
                    sender_prefixes.insert(sender, transactions);
                }
                debug!(
                    target: "engine::tree::txpool_prewarm",
                    parent_hash = ?job.parent_hash,
                    transactions = selection.len(),
                    scanned = selection.scanned,
                    selected_gas = selection.selected_gas,
                    accounts,
                    storage,
                    bytecodes,
                    hinted_transactions,
                    "published txpool prewarming snapshot"
                );
            } else {
                break
            }

            std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
        }

        if !requests.is_closed() &&
            pending.is_none() &&
            source.tracked_block_hash() == job.parent_hash
        {
            pending = Some(job);
        }
    }
}

/// Selects the next bounded batch from a live best-transactions iterator.
///
/// `best_txs` is forward-only and retained for the entire canonical parent. A transaction that
/// would overflow a non-empty batch is deferred rather than discarded, so the next batch resumes
/// at that transaction without reopening the iterator.
fn select_best_transactions<N: NodePrimitives>(
    best_txs: &mut TxPoolPrewarmTransactions<N>,
    deferred: &mut Option<TxPoolPrewarmTransaction<N>>,
    block_gas_limit: u64,
    config: TxPoolPrewarmingConfig,
    warmed: &HashSet<B256>,
    warmed_per_sender: &AddressMap<usize>,
    stop: &AtomicBool,
) -> (TxPoolPrewarmSelection<N>, bool, bool) {
    let gas_limit = block_gas_limit.saturating_mul(config.gas_limit_multiplier);
    let mut selection = TxPoolPrewarmSelection::default();
    let mut selected_per_sender = AddressMap::<usize>::default();

    while selection.scanned < config.max_candidate_scan {
        if stop.load(Ordering::Relaxed) {
            selection.canceled = true;
            break
        }

        let transaction = if let Some(transaction) = deferred.take() {
            Some(transaction)
        } else {
            match catch_unwind(AssertUnwindSafe(|| best_txs.next())) {
                Ok(transaction) => transaction,
                Err(_) => return (selection, true, false),
            }
        };
        let Some(transaction) = transaction else { return (selection, false, true) };
        if warmed.contains(&transaction.hash) {
            selection.scanned += 1;
            continue
        }

        let selected_for_sender = selected_per_sender.entry(transaction.sender).or_default();
        let already_warmed =
            warmed_per_sender.get(&transaction.sender).copied().unwrap_or_default();
        if already_warmed.saturating_add(*selected_for_sender) >= config.max_transactions_per_sender
        {
            selection.scanned += 1;
            continue
        }

        let tx_gas_limit = transaction.transaction.gas_limit();
        if tx_gas_limit > gas_limit {
            selection.scanned += 1;
            continue
        }
        if selection.selected_gas.saturating_add(tx_gas_limit) > gas_limit {
            *deferred = Some(transaction);
            break
        }

        selection.scanned += 1;
        *selected_for_sender += 1;
        selection.selected_gas = selection.selected_gas.saturating_add(tx_gas_limit);
        selection.transactions.push(transaction);
    }

    (selection, false, false)
}

/// Merges fresh transactions into the published nonce-ordered prefixes for affected senders.
///
/// Replaying the bounded prefix lets a later delta observe speculative state produced by earlier
/// transactions from the same sender. A replacement overwrites the transaction at its nonce.
fn transactions_with_prefixes<N: NodePrimitives>(
    prefixes: &AddressMap<Vec<TxPoolPrewarmTransaction<N>>>,
    fresh: &[TxPoolPrewarmTransaction<N>],
) -> AddressMap<Vec<TxPoolPrewarmTransaction<N>>> {
    let mut merged = AddressMap::<Vec<TxPoolPrewarmTransaction<N>>>::default();
    for transaction in fresh {
        let transactions = merged
            .entry(transaction.sender)
            .or_insert_with(|| prefixes.get(&transaction.sender).cloned().unwrap_or_default());
        if let Some(existing) = transactions
            .iter_mut()
            .find(|existing| existing.transaction.nonce() == transaction.transaction.nonce())
        {
            *existing = transaction.clone();
        } else {
            transactions.push(transaction.clone());
        }
    }
    for transactions in merged.values_mut() {
        transactions.sort_unstable_by_key(|transaction| transaction.transaction.nonce());
    }
    merged
}

fn prewarm_transactions<N, P, Evm>(
    evm_config: &Evm,
    job: &TxPoolPrewarmJob<N, P, Evm>,
    cache: &TxPoolPrewarmCache,
    transactions: &AddressMap<Vec<TxPoolPrewarmTransaction<N>>>,
    generation: u64,
    publication: &TxPoolSnapshotPublication,
) -> Option<B256Map<Arc<StateAccessHint>>>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
    Evm: ConfigureEvm<Primitives = N>,
{
    let mut hints = B256Map::default();

    for sender_transactions in transactions.values() {
        if !publication.is_current(generation) {
            return None
        }

        let state_provider = match job.provider_builder.build() {
            Ok(provider) => provider,
            Err(err) => {
                trace!(
                    target: "engine::tree::txpool_prewarm",
                    %err,
                    parent_hash = ?job.parent_hash,
                    "failed to build txpool prewarming state provider"
                );
                return None
            }
        };
        let state_provider = TxPoolPrewarmStateProvider { inner: state_provider, cache };
        let state_provider = StateProviderDatabase::new(state_provider);
        let mut state = State::builder().with_database(state_provider).build();
        let mut evm_env = job.evm_env.clone();
        evm_env.cfg_env.disable_nonce_check = true;
        evm_env.cfg_env.disable_balance_check = true;
        let mut evm = evm_config.evm_with_env(&mut state, evm_env);

        for transaction in sender_transactions {
            if !publication.is_current(generation) {
                return None
            }

            match evm.transact(transaction.transaction.clone()) {
                Ok(result) => {
                    if !publication.is_current(generation) {
                        return None
                    }

                    let (targets, _) = MultiProofTargetsV2::from_state_ref(&result.state);
                    evm.db_mut().commit(result.state);
                    if !targets.is_empty() {
                        hints.insert(transaction.hash, Arc::new(targets.into()));
                    }
                }
                Err(err) => {
                    trace!(
                        target: "engine::tree::txpool_prewarm",
                        %err,
                        tx_hash = ?transaction.hash,
                        sender = %transaction.sender,
                        "speculative txpool transaction execution failed"
                    );
                }
            }
        }
    }

    Some(hints)
}

/// Provider that fills only the reusable txpool-prewarm cache.
struct TxPoolPrewarmStateProvider<'a> {
    inner: StateProviderBox,
    cache: &'a TxPoolPrewarmCache,
}

impl EvmStateProvider for TxPoolPrewarmStateProvider<'_> {
    fn basic_account(
        &self,
        address: &Address,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Account>> {
        self.cache
            .get_or_try_insert_account_with(*address, || self.inner.basic_account(address))
            .map(cached_value)
    }

    fn block_hash(&self, number: BlockNumber) -> reth_errors::ProviderResult<Option<B256>> {
        EvmStateProvider::block_hash(&self.inner, number)
    }

    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        self.cache
            .get_or_try_insert_code_with(*code_hash, || self.inner.bytecode_by_hash(code_hash))
            .map(cached_value)
    }

    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> reth_errors::ProviderResult<Option<StorageValue>> {
        self.cache
            .get_or_try_insert_storage_with(account, storage_key, || {
                self.inner.storage(account, storage_key).map(Option::unwrap_or_default)
            })
            .map(cached_value)
            .map(nonzero_storage_value)
    }
}

fn cached_value<T>(status: reth_execution_cache::CachedStatus<T>) -> T {
    match status {
        reth_execution_cache::CachedStatus::Cached(value) |
        reth_execution_cache::CachedStatus::NotCached(value) => value,
    }
}

fn nonzero_storage_value(value: StorageValue) -> Option<StorageValue> {
    (!value.is_zero()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_primitives::EthPrimitives;

    #[test]
    fn reports_exhausted_best_transactions_iterator() {
        let mut transactions: TxPoolPrewarmTransactions<EthPrimitives> =
            Box::new(std::iter::empty());
        let (selection, panicked, exhausted) = select_best_transactions(
            &mut transactions,
            &mut None,
            30_000_000,
            TxPoolPrewarmingConfig::DEFAULT,
            &HashSet::default(),
            &AddressMap::default(),
            &AtomicBool::new(false),
        );

        assert!(selection.is_empty());
        assert!(!panicked);
        assert!(exhausted);
    }

    fn snapshot(parent_hash: B256, hints: B256Map<Arc<StateAccessHint>>) -> TxPoolPrewarmSnapshot {
        TxPoolPrewarmSnapshot {
            cache: TxPoolPrewarmCache::new().snapshot(parent_hash),
            hints: TxPoolPrewarmHints::new(hints),
        }
    }

    #[test]
    fn hint_snapshot_is_immutable() {
        let first_hash = B256::with_last_byte(1);
        let second_hash = B256::with_last_byte(2);
        let mut hints = B256Map::from_iter([(
            first_hash,
            Arc::new(StateAccessHint {
                accounts: vec![B256::with_last_byte(3)],
                storages: B256Map::default(),
            }),
        )]);

        let snapshot = TxPoolPrewarmHints::snapshot(&hints);
        hints.insert(second_hash, Arc::new(StateAccessHint::default()));

        assert!(snapshot.get(&first_hash).is_some());
        assert!(snapshot.get(&second_hash).is_none());
    }

    #[test]
    fn publication_returns_cache_and_hints_for_matching_parent() {
        let parent_hash = B256::with_last_byte(1);
        let transaction_hash = B256::with_last_byte(2);
        let publication = TxPoolSnapshotPublication::default();
        let generation = publication.begin_head().unwrap();
        let published = snapshot(
            parent_hash,
            B256Map::from_iter([(
                transaction_hash,
                Arc::new(StateAccessHint {
                    accounts: vec![B256::with_last_byte(3)],
                    storages: B256Map::default(),
                }),
            )]),
        );

        assert!(publication.publish(generation, published));
        let (published, _) = publication.cancel_and_snapshot(parent_hash);
        let (cache, hints) = published.unwrap().into_parts();

        assert_eq!(cache.parent_hash(), parent_hash);
        assert!(hints.get(&transaction_hash).is_some());
    }

    #[test]
    fn publication_rejects_snapshot_for_other_parent() {
        let publication = TxPoolSnapshotPublication::default();
        let generation = publication.begin_head().unwrap();

        assert!(
            publication.publish(generation, snapshot(B256::with_last_byte(1), B256Map::default()))
        );
        let (published, _) = publication.cancel_and_snapshot(B256::with_last_byte(2));

        assert!(published.is_none());
    }
}
