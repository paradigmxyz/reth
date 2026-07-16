//! Txpool-driven state prewarming and immutable snapshot publication.

use crate::tree::{
    StateProviderBuilder, StateProviderDatabase, TxPoolPrewarmCache, TxPoolPrewarmCacheSnapshot,
};
use alloy_consensus::transaction::Recovered;
use alloy_evm::Evm;
use alloy_primitives::{Address, BlockNumber, StorageKey, StorageValue, B256};
use reth_evm::{ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::{NodePrimitives, TxTy};
use reth_provider::{BlockReader, StateProviderBox, StateProviderFactory, StateReader};
use reth_revm::{database::EvmStateProvider, db::State};
use std::{
    fmt::Debug,
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
    time::{Duration, Instant},
};
use tracing::{debug, trace};

/// Maximum interval between snapshot publications and delay when no transaction is ready.
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

/// A request to warm txpool transactions against one fully validated parent state.
struct TxPoolPrewarmJob<N: NodePrimitives, P, Evm: ConfigureEvm<Primitives = N>> {
    parent_hash: B256,
    evm_env: EvmEnvFor<Evm>,
    provider_builder: StateProviderBuilder<N, P>,
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

/// Coordinates stale-publication rejection and the latest completed snapshot.
#[derive(Debug, Default)]
struct TxPoolSnapshotPublication {
    generation: AtomicU64,
    paused: AtomicBool,
    snapshot: RwLock<Option<TxPoolPrewarmCacheSnapshot>>,
}

impl TxPoolSnapshotPublication {
    fn begin_head(&self) -> Option<u64> {
        let mut snapshot =
            self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        if self.paused.load(Ordering::Acquire) {
            return None
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *snapshot = None;
        Some(generation)
    }

    fn resume_head(&self) -> Option<u64> {
        let _snapshot = self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        if self.paused.load(Ordering::Acquire) {
            return None
        }
        Some(self.generation.fetch_add(1, Ordering::AcqRel) + 1)
    }

    fn cancel(&self) -> u64 {
        let _snapshot = self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        self.paused.store(true, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn snapshot(&self, parent_hash: B256) -> Option<TxPoolPrewarmCacheSnapshot> {
        self.snapshot
            .read()
            .expect("txpool snapshot publication lock poisoned")
            .as_ref()
            .filter(|snapshot| snapshot.parent_hash() == parent_hash)
            .cloned()
    }

    fn publish(&self, generation: u64, snapshot: TxPoolPrewarmCacheSnapshot) -> bool {
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

    /// Returns the latest fully published snapshot for `parent_hash`.
    pub(crate) fn snapshot(&self, parent_hash: B256) -> Option<TxPoolPrewarmCacheSnapshot> {
        self.publication.snapshot(parent_hash)
    }

    /// Starts continuous warming for the latest canonical head.
    pub(crate) fn start(
        &self,
        parent_hash: B256,
        evm_env: EvmEnvFor<Evm>,
        provider_builder: StateProviderBuilder<N, P>,
    ) {
        self.requests.replace(TxPoolPrewarmJob { parent_hash, evm_env, provider_builder });
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
    publication: Arc<TxPoolSnapshotPublication>,
    activity: Arc<TxPoolWorkerActivity>,
) where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    let cache = TxPoolPrewarmCache::default();
    let mut best_txs: Option<TxPoolPrewarmTransactions<N>> = None;
    let mut best_txs_parent_hash = None;
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
            let Some(opened) = source.best_transactions(job.parent_hash) else {
                pending = Some(job);
                std::thread::sleep(TXPOOL_HEAD_POLL_INTERVAL);
                continue
            };
            best_txs = Some(opened);
            best_txs_parent_hash = Some(job.parent_hash);
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

            let Some(activity_guard) = activity.try_enter(&publication, active_generation) else {
                break
            };
            let (completed, transaction_count) = prewarm_transactions(
                &evm_config,
                &job,
                &cache,
                best_txs.as_mut().expect("best transactions opened for active parent"),
                Instant::now() + TXPOOL_REFRESH_INTERVAL,
                active_generation,
                &publication,
            );
            drop(activity_guard);

            if !completed {
                if publication.is_current(active_generation) && !publication.is_paused() {
                    std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                }
                break
            }
            if !publication.is_current(active_generation) {
                break
            }
            if transaction_count == 0 {
                std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                continue
            }

            // The deep clone happens privately. Only the short pointer swap below is serialized
            // with `newPayload` snapshot acquisition.
            let snapshot = cache.snapshot(job.parent_hash);
            let (accounts, storage, bytecodes) = snapshot.entry_counts();
            if publication.publish(active_generation, snapshot) {
                debug!(
                    target: "engine::tree::txpool_prewarm",
                    parent_hash = ?job.parent_hash,
                    transactions = transaction_count,
                    accounts,
                    storage,
                    bytecodes,
                    "published txpool prewarming snapshot"
                );
            } else {
                break
            }
        }

        if !requests.is_closed() &&
            pending.is_none() &&
            source.tracked_block_hash() == job.parent_hash
        {
            pending = Some(job);
        }
    }
}

fn prewarm_transactions<N, P, Evm, I>(
    evm_config: &Evm,
    job: &TxPoolPrewarmJob<N, P, Evm>,
    cache: &TxPoolPrewarmCache,
    transactions: I,
    deadline: Instant,
    generation: u64,
    publication: &TxPoolSnapshotPublication,
) -> (bool, usize)
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
    Evm: ConfigureEvm<Primitives = N>,
    I: IntoIterator<Item = TxPoolPrewarmTransaction<N>>,
{
    let state_provider = match job.provider_builder.build() {
        Ok(provider) => provider,
        Err(err) => {
            trace!(
                target: "engine::tree::txpool_prewarm",
                %err,
                parent_hash = ?job.parent_hash,
                "failed to build txpool prewarming state provider"
            );
            return (false, 0)
        }
    };
    let state_provider = TxPoolPrewarmStateProvider { inner: state_provider, cache };
    let state_provider = StateProviderDatabase::new(state_provider);
    let mut state = State::builder().with_database(state_provider).build();
    let mut evm_env = job.evm_env.clone();
    evm_env.cfg_env.disable_nonce_check = true;
    evm_env.cfg_env.disable_balance_check = true;
    let mut evm = evm_config.evm_with_env(&mut state, evm_env);

    let mut transaction_count = 0;
    for transaction in transactions {
        if !publication.is_current(generation) {
            return (false, transaction_count)
        }
        transaction_count += 1;

        if let Err(err) = evm.transact(transaction.transaction) {
            trace!(
                target: "engine::tree::txpool_prewarm",
                %err,
                tx_hash = ?transaction.hash,
                sender = %transaction.sender,
                "speculative txpool transaction execution failed"
            );
        }
        if Instant::now() >= deadline {
            break
        }
    }

    (true, transaction_count)
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

    #[test]
    fn reading_snapshot_does_not_interrupt_publication() {
        let publication = TxPoolSnapshotPublication::default();
        let generation = publication.begin_head().expect("publication should be active");
        let parent_hash = B256::repeat_byte(0x01);
        let snapshot = TxPoolPrewarmCache::default().snapshot(parent_hash);
        assert!(publication.publish(generation, snapshot));

        assert_eq!(
            publication.snapshot(parent_hash).map(|snapshot| snapshot.parent_hash()),
            Some(parent_hash)
        );
        assert!(publication.snapshot(B256::ZERO).is_none());
        assert!(publication.is_current(generation));
        assert!(!publication.is_paused());
    }
}
