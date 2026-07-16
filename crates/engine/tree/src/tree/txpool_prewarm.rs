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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use tracing::{debug, trace};

/// Maximum interval between snapshot publications and delay when no transaction is ready.
const TXPOOL_REFRESH_INTERVAL: Duration = Duration::from_millis(100);

/// Delay while waiting for pool maintenance to advance to the state being warmed.
const TXPOOL_HEAD_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A live, forward-only view of the pool's best transactions for one canonical parent.
///
/// Returning [`None`](Iterator::next) only means no transaction is currently ready. The same
/// iterator can yield transactions that become pending later.
pub type TxPoolPrewarmTransactions<N> =
    Box<dyn Iterator<Item = TxPoolPrewarmTransaction<N>> + Send>;

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

/// Source of txpool transactions for best-effort cache prewarming.
pub trait TxPoolPrewarmSource<N: NodePrimitives>: Send + Sync + Debug {
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

/// Coordinates snapshot publication and worker quiescence.
#[derive(Debug, Default)]
struct TxPoolSnapshotPublication {
    paused: AtomicBool,
    state: Mutex<TxPoolSnapshotPublicationState>,
    idle: Condvar,
}

#[derive(Debug, Default)]
struct TxPoolSnapshotPublicationState {
    active: bool,
    snapshot: Option<TxPoolPrewarmCacheSnapshot>,
}

/// Keeps txpool prewarming paused until the cache-sensitive work is complete.
#[derive(Debug)]
pub(super) struct TxPoolPrewarmPauseGuard {
    publication: Arc<TxPoolSnapshotPublication>,
    wait_duration: Duration,
}

impl TxPoolPrewarmPauseGuard {
    pub(super) const fn wait_duration(&self) -> Duration {
        self.wait_duration
    }
}

impl Drop for TxPoolPrewarmPauseGuard {
    fn drop(&mut self) {
        self.publication.resume();
    }
}

impl TxPoolSnapshotPublication {
    fn begin_head(&self) -> bool {
        let mut state = self.state.lock().expect("txpool snapshot publication lock poisoned");
        if self.paused.load(Ordering::Acquire) {
            return false
        }
        state.snapshot = None;
        true
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }

    fn pause_and_wait(self: &Arc<Self>) -> TxPoolPrewarmPauseGuard {
        let start = Instant::now();
        self.pause();
        let mut state = self.state.lock().expect("txpool snapshot publication lock poisoned");
        while state.active {
            state = self.idle.wait(state).expect("txpool snapshot publication lock poisoned");
        }
        TxPoolPrewarmPauseGuard { publication: Arc::clone(self), wait_duration: start.elapsed() }
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::Release);
    }

    fn try_enter(self: &Arc<Self>) -> Option<TxPoolPrewarmWaveGuard> {
        let mut state = self.state.lock().expect("txpool snapshot publication lock poisoned");
        if state.active || self.paused.load(Ordering::Acquire) {
            return None
        }
        state.active = true;
        Some(TxPoolPrewarmWaveGuard { publication: Arc::clone(self), active: true })
    }

    fn finish_wave(&self, snapshot: Option<TxPoolPrewarmCacheSnapshot>) -> bool {
        let mut state = self.state.lock().expect("txpool snapshot publication lock poisoned");
        let published = if self.paused.load(Ordering::Acquire) {
            false
        } else if let Some(snapshot) = snapshot {
            state.snapshot = Some(snapshot);
            true
        } else {
            false
        };
        state.active = false;
        self.idle.notify_all();
        published
    }

    fn snapshot(&self, parent_hash: B256) -> Option<TxPoolPrewarmCacheSnapshot> {
        self.state
            .lock()
            .expect("txpool snapshot publication lock poisoned")
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.parent_hash() == parent_hash)
            .cloned()
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
}

struct TxPoolPrewarmWaveGuard {
    publication: Arc<TxPoolSnapshotPublication>,
    active: bool,
}

impl TxPoolPrewarmWaveGuard {
    fn publish(mut self, snapshot: TxPoolPrewarmCacheSnapshot) -> bool {
        let published = self.publication.finish_wave(Some(snapshot));
        self.active = false;
        published
    }
}

impl Drop for TxPoolPrewarmWaveGuard {
    fn drop(&mut self) {
        if self.active {
            self.publication.finish_wave(None);
        }
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
}

impl<N, P, Evm> Debug for TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state =
            self.publication.state.lock().expect("txpool snapshot publication lock poisoned");
        f.debug_struct("TxPoolPrewarmHandle")
            .field("paused", &self.publication.is_paused())
            .field("active", &state.active)
            .field("published", &state.snapshot.as_ref().map(|s| s.parent_hash()))
            .finish_non_exhaustive()
    }
}

impl<N, P, Evm> TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Cancels speculative work and waits for any provider-backed EVM call to release its state.
    pub(crate) fn cancel_and_wait(&self) -> TxPoolPrewarmPauseGuard {
        self.publication.pause_and_wait()
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

        runtime.spawn_blocking_named("txpool-prewarm", move || {
            txpool_prewarm_loop(worker_requests, source, evm_config, worker_publication)
        });

        Self { requests, publication }
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
        self.publication.pause();
        self.requests.close();
    }
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
        self.cache.get_or_try_insert_account_with(*address, || self.inner.basic_account(address))
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
            .map(|value| (!value.is_zero()).then_some(value))
    }
}

fn txpool_prewarm_loop<N, P, Evm>(
    requests: Arc<TxPoolPrewarmRequests<TxPoolPrewarmJob<N, P, Evm>>>,
    source: Arc<dyn TxPoolPrewarmSource<N>>,
    evm_config: Evm,
    publication: Arc<TxPoolSnapshotPublication>,
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

        let job = pending.take().expect("canonical request exists");

        if best_txs_parent_hash != Some(job.parent_hash) {
            let Some(opened) = source.best_transactions(job.parent_hash) else {
                pending = Some(job);
                std::thread::sleep(TXPOOL_HEAD_POLL_INTERVAL);
                continue
            };
            best_txs = Some(opened);
            best_txs_parent_hash = Some(job.parent_hash);
        }

        // Wait for pool maintenance to catch up to the authoritative canonical-head request
        // before invalidating the previous publication.
        let new_parent = cache_parent_hash != Some(job.parent_hash);
        if new_parent && !publication.begin_head() {
            if pending.is_none() {
                pending = Some(job);
            }
            continue
        }
        if new_parent {
            cache.clear();
            cache_parent_hash = Some(job.parent_hash);
        }
        debug!(
            target: "engine::tree::txpool_prewarm",
            parent_hash = ?job.parent_hash,
            "started txpool prewarming"
        );

        while !publication.is_paused() {
            if let Some(request) = requests.take() {
                pending = Some(request);
                break
            }
            if requests.is_closed() || publication.is_paused() {
                break
            }

            let Some(activity_guard) = publication.try_enter() else { break };
            let (completed, transaction_count) = prewarm_transactions(
                &evm_config,
                &job,
                &cache,
                best_txs.as_mut().expect("best transactions opened for active parent"),
                Instant::now() + TXPOOL_REFRESH_INTERVAL,
                &publication,
            );

            if !completed {
                drop(activity_guard);
                if !publication.is_paused() {
                    std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                }
                break
            }
            if transaction_count == 0 {
                drop(activity_guard);
                std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                continue
            }

            let snapshot = cache.snapshot(job.parent_hash);
            let (accounts, storage, bytecodes) = snapshot.entry_counts();
            if activity_guard.publish(snapshot) {
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

        if !requests.is_closed() && pending.is_none() {
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
        if publication.is_paused() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_snapshot_does_not_interrupt_publication() {
        let publication = Arc::new(TxPoolSnapshotPublication::default());
        assert!(publication.begin_head());
        let activity_guard = publication.try_enter().expect("publication should be active");
        let parent_hash = B256::repeat_byte(0x01);
        let snapshot = TxPoolPrewarmCache::default().snapshot(parent_hash);
        assert!(activity_guard.publish(snapshot));

        assert_eq!(
            publication.snapshot(parent_hash).map(|snapshot| snapshot.parent_hash()),
            Some(parent_hash)
        );
        assert!(publication.snapshot(B256::ZERO).is_none());
        assert!(!publication.is_paused());
    }

    #[test]
    fn paused_wave_is_not_published() {
        let publication = Arc::new(TxPoolSnapshotPublication::default());
        assert!(publication.begin_head());
        let activity_guard = publication.try_enter().expect("publication should be active");

        publication.pause();
        let parent_hash = B256::repeat_byte(0x01);
        let snapshot = TxPoolPrewarmCache::default().snapshot(parent_hash);
        assert!(!activity_guard.publish(snapshot));
        assert!(publication.snapshot(parent_hash).is_none());
    }

    #[test]
    fn pause_guard_resumes_publication_on_drop() {
        let publication = Arc::new(TxPoolSnapshotPublication::default());

        let guard = publication.pause_and_wait();
        assert!(publication.is_paused());

        drop(guard);
        assert!(!publication.is_paused());
    }
}
