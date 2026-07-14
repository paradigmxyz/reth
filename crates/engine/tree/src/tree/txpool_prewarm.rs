//! Txpool-driven state prewarming and immutable snapshot publication.

use crate::tree::{
    StateProviderBuilder, StateProviderDatabase, TxPoolPrewarmCache, TxPoolPrewarmCacheSnapshot,
};
use alloy_consensus::{transaction::Recovered, Transaction as _};
use alloy_evm::Evm;
use alloy_primitives::{
    keccak256,
    map::{AddressMap, B256Map, HashSet},
    Address, BlockNumber, StorageKey, StorageValue, B256,
};
use metrics::{Counter, Histogram};
use reth_engine_primitives::TxPoolPrewarmingConfig;
use reth_evm::{ConfigureEvm, EvmEnvFor};
use reth_metrics::Metrics;
use reth_primitives_traits::{NodePrimitives, TxTy};
use reth_provider::{BlockReader, StateProviderBox, StateProviderFactory, StateReader};
use reth_revm::{database::EvmStateProvider, db::State};
use reth_trie::{MultiProofTargetsV2, ProofV2Target};
use revm::{state::EvmState, DatabaseCommit};
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

/// Maximum changed trie targets retained from one speculative execution wave.
const TXPOOL_TRIE_PREWARM_MAX_TARGETS_PER_WAVE: usize = 300;

/// Maximum cumulative proof calculation time spent warming one canonical parent.
const TXPOOL_TRIE_PREWARM_PARENT_BUDGET: Duration = Duration::from_secs(1);

/// Stop the current request if one non-cancellable proof chunk exceeds this duration.
const TXPOOL_TRIE_PREWARM_SLOW_CHUNK: Duration = Duration::from_millis(100);

/// Targets passed to one non-cancellable background proof calculation.
const TXPOOL_TRIE_PREWARM_CHUNK_SIZE: usize = 5;

type TxPoolPrewarmMarker<N, P, Evm> = PhantomData<fn() -> (N, P, Evm)>;

type TxPoolTrieProofCalculator = dyn Fn(MultiProofTargetsV2) -> Result<usize, String> + Send + Sync;

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

/// A request to warm txpool transactions against one fully validated parent state.
struct TxPoolPrewarmJob<N: NodePrimitives, P, Evm: ConfigureEvm<Primitives = N>> {
    parent_hash: B256,
    head_epoch: u64,
    evm_env: EvmEnvFor<Evm>,
    provider_builder: StateProviderBuilder<N, P>,
    block_gas_limit: u64,
    trie_proof_calculator: Arc<TxPoolTrieProofCalculator>,
}

/// A bounded unit of exact-parent trie page warming.
struct TxPoolTriePrewarmRequest {
    parent_hash: B256,
    head_epoch: u64,
    epoch: u64,
    targets: MultiProofTargetsV2,
    calculator: Arc<TxPoolTrieProofCalculator>,
}

/// Deduplicated changed paths collected from one speculative execution wave.
#[derive(Debug, Default)]
struct TxPoolTriePrewarmTargets {
    accounts: Vec<B256>,
    storages: B256Map<Vec<B256>>,
}

impl TxPoolTriePrewarmTargets {
    fn extend_state(&mut self, state: &EvmState) {
        for (address, account) in state {
            if !account.is_touched() {
                continue
            }

            let hashed_address = keccak256(address);
            if account.is_selfdestructed() {
                self.insert_account(hashed_address);
                continue
            }

            if account.info != account.original_info() {
                self.insert_account(hashed_address);
            }

            for (slot, value) in &account.storage {
                if value.is_changed() {
                    self.insert_storage(hashed_address, keccak256(B256::new(slot.to_be_bytes())));
                }
            }
        }
    }

    fn insert_account(&mut self, account: B256) {
        if !self.accounts.contains(&account) &&
            self.len() < TXPOOL_TRIE_PREWARM_MAX_TARGETS_PER_WAVE
        {
            self.accounts.push(account);
        }
    }

    fn insert_storage(&mut self, account: B256, slot: B256) {
        // Direct V2 proof calculation does not promote storage targets into the account trie.
        // Include the account path so the calculator reads the storage root first.
        self.insert_account(account);
        if !self.accounts.contains(&account) {
            return
        }

        let already_present =
            self.storages.get(&account).is_some_and(|slots| slots.contains(&slot));
        if !already_present && self.len() < TXPOOL_TRIE_PREWARM_MAX_TARGETS_PER_WAVE {
            self.storages.entry(account).or_default().push(slot);
        }
    }

    fn len(&self) -> usize {
        self.accounts.len() + self.storages.values().map(Vec::len).sum::<usize>()
    }

    fn extend(&mut self, other: Self) {
        for account in other.accounts {
            self.insert_account(account);
            if let Some(slots) = other.storages.get(&account) {
                for slot in slots {
                    self.insert_storage(account, *slot);
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.storages.is_empty()
    }

    fn into_multiproof_targets(self) -> MultiProofTargetsV2 {
        MultiProofTargetsV2 {
            account_targets: self.accounts.into_iter().map(ProofV2Target::new).collect(),
            storage_targets: self
                .storages
                .into_iter()
                .map(|(address, slots)| {
                    (address, slots.into_iter().map(ProofV2Target::new).collect())
                })
                .collect(),
        }
    }
}

#[derive(Clone, Metrics)]
#[metrics(scope = "sync.txpool_trie_prewarm")]
struct TxPoolTriePrewarmMetrics {
    /// Duration of one bounded V2 proof calculation in seconds.
    proof_duration_seconds: Histogram,
    /// Number of targets in one bounded V2 proof calculation.
    proof_targets: Histogram,
    /// Number of nodes returned by one bounded V2 proof calculation.
    returned_proof_nodes: Histogram,
    /// Number of failed or panicked proof calculations.
    proof_errors: Counter,
    /// Time validation spent waiting for an active proof calculation to release its provider.
    validation_wait_duration_seconds: Histogram,
}

/// Single pending request slot shared with a worker.
struct TxPoolPrewarmRequests<J> {
    latest: Mutex<Option<J>>,
    available: Condvar,
    closed: AtomicBool,
    minimum_epoch: AtomicU64,
}

impl<J> TxPoolPrewarmRequests<J> {
    const fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            available: Condvar::new(),
            closed: AtomicBool::new(false),
            minimum_epoch: AtomicU64::new(0),
        }
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

impl TxPoolPrewarmRequests<TxPoolTriePrewarmRequest> {
    /// Preserves the oldest queued wave for one epoch, but replaces stale epochs.
    fn try_enqueue(&self, request: TxPoolTriePrewarmRequest) -> bool {
        let mut pending = self.latest.lock().expect("txpool prewarm request lock poisoned");
        if self.closed.load(Ordering::Acquire) ||
            request.epoch < self.minimum_epoch.load(Ordering::Acquire)
        {
            return false
        }

        match pending.as_ref() {
            None => *pending = Some(request),
            Some(current) if current.epoch < request.epoch => *pending = Some(request),
            Some(_) => return false,
        }
        self.available.notify_one();
        true
    }

    fn advance_epoch(&self, epoch: u64) {
        let mut pending = self.latest.lock().expect("txpool prewarm request lock poisoned");
        let minimum_epoch = self.minimum_epoch.load(Ordering::Relaxed).max(epoch);
        self.minimum_epoch.store(minimum_epoch, Ordering::Release);
        if pending.as_ref().is_some_and(|request| request.epoch < minimum_epoch) {
            pending.take();
        }
    }
}

/// Linearization point shared by worker publication and payload snapshot acquisition.
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

    fn cancel_and_snapshot(&self, parent_hash: B256) -> (Option<TxPoolPrewarmCacheSnapshot>, u64) {
        let snapshot = self.snapshot.write().expect("txpool snapshot publication lock poisoned");
        self.paused.store(true, Ordering::Release);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        (
            snapshot.as_ref().filter(|snapshot| snapshot.parent_hash() == parent_hash).cloned(),
            generation,
        )
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

    fn try_enter_trie(
        self: &Arc<Self>,
        head_epoch: &AtomicU64,
        expected_head_epoch: u64,
        epoch: &AtomicU64,
        expected_epoch: u64,
    ) -> Option<TxPoolWorkerActivityGuard> {
        let mut active = self.active.lock().expect("txpool worker activity lock poisoned");
        if *active ||
            head_epoch.load(Ordering::Acquire) != expected_head_epoch ||
            epoch.load(Ordering::Acquire) != expected_epoch
        {
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

struct TxPoolTriePrewarmState {
    requests: TxPoolPrewarmRequests<TxPoolTriePrewarmRequest>,
    activity: Arc<TxPoolWorkerActivity>,
    current_head: Mutex<Option<B256>>,
    head_epoch: AtomicU64,
    epoch: AtomicU64,
    metrics: TxPoolTriePrewarmMetrics,
}

impl TxPoolTriePrewarmState {
    fn new() -> Self {
        Self {
            requests: TxPoolPrewarmRequests::new(),
            activity: Arc::new(TxPoolWorkerActivity::default()),
            current_head: Mutex::new(None),
            head_epoch: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            metrics: TxPoolTriePrewarmMetrics::default(),
        }
    }

    fn advance_epoch(&self) -> u64 {
        let epoch = {
            let _active =
                self.activity.active.lock().expect("txpool worker activity lock poisoned");
            self.epoch.fetch_add(1, Ordering::SeqCst) + 1
        };
        self.requests.advance_epoch(epoch);
        epoch
    }

    fn begin_head(&self, parent_hash: B256) -> u64 {
        let mut current_head = self.current_head.lock().expect("txpool trie head lock poisoned");
        if *current_head == Some(parent_hash) {
            return self.head_epoch.load(Ordering::Acquire)
        }

        let (head_epoch, epoch) = {
            let _active =
                self.activity.active.lock().expect("txpool worker activity lock poisoned");
            *current_head = Some(parent_hash);
            (
                self.head_epoch.fetch_add(1, Ordering::SeqCst) + 1,
                self.epoch.fetch_add(1, Ordering::SeqCst) + 1,
            )
        };
        self.requests.advance_epoch(epoch);
        head_epoch
    }

    fn try_enqueue(&self, request: TxPoolTriePrewarmRequest) -> bool {
        if self.head_epoch.load(Ordering::Acquire) != request.head_epoch {
            return false
        }
        self.requests.try_enqueue(request)
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

/// Coordinates the long-lived workers and the latest completed immutable snapshot.
pub(crate) struct TxPoolPrewarmHandle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    requests: Arc<TxPoolPrewarmRequests<TxPoolPrewarmJob<N, P, Evm>>>,
    trie: Arc<TxPoolTriePrewarmState>,
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
    fn advance_trie_epoch(&self) -> u64 {
        self.trie.advance_epoch()
    }

    /// Returns the proof epoch for `parent_hash`, invalidating work for a different head.
    pub(crate) fn begin_trie_head(&self, parent_hash: B256) -> u64 {
        self.trie.begin_head(parent_hash)
    }

    /// Cancels speculative work and waits for provider-backed EVM and trie calls to finish.
    pub(crate) fn cancel_and_wait(&self) -> Duration {
        let start = Instant::now();
        let generation = self.publication.cancel();
        self.advance_trie_epoch();
        self.wait_generation.store(generation, Ordering::Release);
        self.activity.wait_until_idle();
        self.trie.activity.wait_until_idle();
        start.elapsed()
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
    /// Spawns the long-lived workers. The EVM worker owns its mutable cache and clears it between
    /// heads without releasing map capacity.
    pub(crate) fn spawn(
        runtime: &reth_tasks::Runtime,
        source: Arc<dyn TxPoolPrewarmSource<N>>,
        evm_config: Evm,
        config: TxPoolPrewarmingConfig,
    ) -> Self {
        let requests = Arc::new(TxPoolPrewarmRequests::new());
        let worker_requests = Arc::clone(&requests);
        let trie = Arc::new(TxPoolTriePrewarmState::new());
        let worker_trie = Arc::clone(&trie);
        let txpool_trie = Arc::clone(&trie);
        let publication = Arc::new(TxPoolSnapshotPublication::default());
        let worker_publication = Arc::clone(&publication);
        let activity = Arc::new(TxPoolWorkerActivity::default());
        let worker_activity = Arc::clone(&activity);

        runtime.spawn_blocking_named("txpool-trie-prewarm", move || {
            txpool_trie_prewarm_loop(worker_trie, TXPOOL_TRIE_PREWARM_CHUNK_SIZE)
        });

        runtime.spawn_blocking_named("txpool-prewarm", move || {
            txpool_prewarm_loop(
                worker_requests,
                txpool_trie,
                source,
                evm_config,
                config,
                worker_publication,
                worker_activity,
            )
        });

        Self {
            requests,
            trie,
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
    ) -> (Option<TxPoolPrewarmCacheSnapshot>, TxPoolPrewarmPauseGuard) {
        let (snapshot, generation) = self.publication.cancel_and_snapshot(parent_hash);
        self.advance_trie_epoch();
        let wait = self.trie.activity.wait_until_idle();
        self.trie.metrics.validation_wait_duration_seconds.record(wait.as_secs_f64());
        if wait.as_millis() > 5 {
            debug!(
                target: "engine::tree::txpool_prewarm",
                ?wait,
                "waited for txpool trie page prewarming to stop"
            );
        }
        (
            snapshot,
            TxPoolPrewarmPauseGuard { publication: Arc::clone(&self.publication), generation },
        )
    }

    /// Starts continuous warming for the latest canonical head.
    pub(crate) fn start(
        &self,
        parent_hash: B256,
        head_epoch: u64,
        evm_env: EvmEnvFor<Evm>,
        provider_builder: StateProviderBuilder<N, P>,
        block_gas_limit: u64,
        calculate_trie_proof: impl Fn(MultiProofTargetsV2) -> Result<usize, String>
            + Send
            + Sync
            + 'static,
    ) {
        let trie_proof_calculator = Arc::new(calculate_trie_proof);
        self.requests.replace(TxPoolPrewarmJob {
            parent_hash,
            head_epoch,
            evm_env,
            provider_builder,
            block_gas_limit,
            trie_proof_calculator,
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
        self.advance_trie_epoch();
        self.requests.close();
        self.trie.requests.close();
    }
}

fn txpool_trie_prewarm_loop(trie: Arc<TxPoolTriePrewarmState>, chunk_size: usize) {
    let mut budget_parent = None;
    let mut parent_spent = Duration::ZERO;

    while !trie.requests.is_closed() {
        let Some(request) = trie.requests.wait(None) else { continue };
        if trie.requests.is_closed() ||
            trie.head_epoch.load(Ordering::Acquire) != request.head_epoch ||
            trie.epoch.load(Ordering::Acquire) != request.epoch
        {
            continue
        }
        if budget_parent != Some(request.parent_hash) {
            budget_parent = Some(request.parent_hash);
            parent_spent = Duration::ZERO;
        }
        if parent_spent >= TXPOOL_TRIE_PREWARM_PARENT_BUDGET {
            continue
        }

        let chunks = txpool_trie_prewarm_chunks(request.targets, chunk_size.max(1));

        for chunk in chunks {
            if trie.requests.is_closed() ||
                trie.head_epoch.load(Ordering::Acquire) != request.head_epoch ||
                trie.epoch.load(Ordering::Acquire) != request.epoch
            {
                break
            }

            let Some(activity_guard) = trie.activity.try_enter_trie(
                &trie.head_epoch,
                request.head_epoch,
                &trie.epoch,
                request.epoch,
            ) else {
                break
            };
            let target_count = chunk.chunking_length();
            let started_at = Instant::now();
            let result = catch_unwind(AssertUnwindSafe(|| (request.calculator)(chunk)));
            let elapsed = started_at.elapsed();
            parent_spent = parent_spent.saturating_add(elapsed);

            match result {
                Ok(Ok(returned_proof_nodes)) => {
                    trie.metrics.proof_duration_seconds.record(elapsed.as_secs_f64());
                    trie.metrics.proof_targets.record(target_count as f64);
                    trie.metrics.returned_proof_nodes.record(returned_proof_nodes as f64);
                    trace!(
                        target: "engine::tree::txpool_prewarm",
                        parent_hash = ?request.parent_hash,
                        target_count,
                        returned_proof_nodes,
                        ?elapsed,
                        "warmed trie pages from speculative txpool targets"
                    );
                }
                Ok(Err(err)) => {
                    trie.metrics.proof_errors.increment(1);
                    trace!(
                        target: "engine::tree::txpool_prewarm",
                        parent_hash = ?request.parent_hash,
                        %err,
                        "failed to warm trie pages from speculative txpool targets"
                    );
                    drop(activity_guard);
                    break
                }
                Err(_) => {
                    trie.metrics.proof_errors.increment(1);
                    warn!(
                        target: "engine::tree::txpool_prewarm",
                        parent_hash = ?request.parent_hash,
                        "txpool trie page prewarming panicked"
                    );
                    drop(activity_guard);
                    break
                }
            }
            drop(activity_guard);

            // A single account proof can synchronously calculate its storage root. Stop this
            // request after a slow chunk so speculative work does not keep competing with the
            // next payload or persistence operation.
            if elapsed > TXPOOL_TRIE_PREWARM_SLOW_CHUNK {
                debug!(
                    target: "engine::tree::txpool_prewarm",
                    parent_hash = ?request.parent_hash,
                    ?elapsed,
                    "stopping slow txpool trie page prewarming request"
                );
                break
            }
            if parent_spent >= TXPOOL_TRIE_PREWARM_PARENT_BUDGET {
                debug!(
                    target: "engine::tree::txpool_prewarm",
                    parent_hash = ?request.parent_hash,
                    ?parent_spent,
                    "exhausted txpool trie page prewarming budget"
                );
                break
            }
        }
    }
}

/// Splits targets so one non-cancellable calculation contains at most one account proof.
///
/// Encoding an account can synchronously calculate its storage root. Keeping account targets
/// separate limits each non-cancellable call to one such calculation.
fn txpool_trie_prewarm_chunks(
    targets: MultiProofTargetsV2,
    chunk_size: usize,
) -> Vec<MultiProofTargetsV2> {
    let MultiProofTargetsV2 { account_targets, mut storage_targets } = targets;
    let mut chunks = Vec::new();

    for account in account_targets {
        let address = account.key();
        let slots = storage_targets.remove(&address).unwrap_or_default();
        push_account_chunks(&mut chunks, account, address, slots, chunk_size);
    }

    // Direct proof calculation does not promote storage-only targets into the account trie. Keep
    // this defensive path even though the speculative target collector already performs that
    // promotion.
    for (address, slots) in storage_targets {
        push_account_chunks(&mut chunks, ProofV2Target::new(address), address, slots, chunk_size);
    }

    chunks
}

fn push_account_chunks(
    chunks: &mut Vec<MultiProofTargetsV2>,
    account: ProofV2Target,
    address: B256,
    slots: Vec<ProofV2Target>,
    chunk_size: usize,
) {
    let mut slots = slots.into_iter();
    let first_slots = slots.by_ref().take(chunk_size.saturating_sub(1)).collect::<Vec<_>>();
    chunks.push(MultiProofTargetsV2 {
        account_targets: vec![account],
        storage_targets: if first_slots.is_empty() {
            B256Map::default()
        } else {
            B256Map::from_iter([(address, first_slots)])
        },
    });

    loop {
        let slots = slots.by_ref().take(chunk_size).collect::<Vec<_>>();
        if slots.is_empty() {
            break
        }
        chunks.push(MultiProofTargetsV2 {
            account_targets: Vec::new(),
            storage_targets: B256Map::from_iter([(address, slots)]),
        });
    }
}

fn txpool_prewarm_loop<N, P, Evm>(
    requests: Arc<TxPoolPrewarmRequests<TxPoolPrewarmJob<N, P, Evm>>>,
    trie: Arc<TxPoolTriePrewarmState>,
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
        if trie.head_epoch.load(Ordering::Acquire) != job.head_epoch {
            continue
        }

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
        let active_trie_epoch = trie.epoch.load(Ordering::Acquire);
        if new_parent {
            cache.clear();
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

        while publication.is_current(active_generation) &&
            !publication.is_paused() &&
            trie.head_epoch.load(Ordering::Acquire) == job.head_epoch
        {
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
                let (selection, cursor_panicked) = select_best_transactions(
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
            let fresh_transaction_hashes = inflight
                .as_ref()
                .expect("non-empty selection is in flight")
                .transactions
                .iter()
                .map(|transaction| transaction.hash)
                .collect::<HashSet<_>>();
            let Some(activity_guard) = activity.try_enter(&publication, active_generation) else {
                break
            };
            let trie_targets = catch_unwind(AssertUnwindSafe(|| {
                prewarm_transactions(
                    &evm_config,
                    &job,
                    &cache,
                    &sender_transactions,
                    &fresh_transaction_hashes,
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

            let Some(trie_targets) = trie_targets else {
                if publication.is_current(active_generation) && !publication.is_paused() {
                    std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
                }
                break
            };
            if !publication.is_current(active_generation) {
                break
            }

            // The deep clone happens privately. Only the short pointer swap below is serialized
            // with `newPayload` snapshot acquisition.
            let snapshot = cache.snapshot(job.parent_hash);
            let (accounts, storage, bytecodes) = snapshot.entry_counts();
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
                    "published txpool prewarming snapshot"
                );

                if !trie_targets.is_empty() {
                    let request = TxPoolTriePrewarmRequest {
                        parent_hash: job.parent_hash,
                        head_epoch: job.head_epoch,
                        epoch: active_trie_epoch,
                        targets: trie_targets.into_multiproof_targets(),
                        calculator: Arc::clone(&job.trie_proof_calculator),
                    };
                    if !trie.try_enqueue(request) {
                        trace!(
                            target: "engine::tree::txpool_prewarm",
                            parent_hash = ?job.parent_hash,
                            "dropped txpool trie page prewarming wave"
                        );
                    }
                }
            } else {
                break
            }

            std::thread::sleep(TXPOOL_REFRESH_INTERVAL);
        }

        if !requests.is_closed() &&
            pending.is_none() &&
            trie.head_epoch.load(Ordering::Acquire) == job.head_epoch &&
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
) -> (TxPoolPrewarmSelection<N>, bool) {
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
                Err(_) => return (selection, true),
            }
        };
        let Some(transaction) = transaction else { break };
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

    (selection, false)
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
    fresh_transaction_hashes: &HashSet<B256>,
    generation: u64,
    publication: &TxPoolSnapshotPublication,
) -> Option<TxPoolTriePrewarmTargets>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
    Evm: ConfigureEvm<Primitives = N>,
{
    let mut fresh_targets = TxPoolTriePrewarmTargets::default();
    let mut replayed_targets = TxPoolTriePrewarmTargets::default();

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
                    if fresh_transaction_hashes.contains(&transaction.hash) {
                        fresh_targets.extend_state(&result.state);
                    } else {
                        replayed_targets.extend_state(&result.state);
                    }
                    evm.db_mut().commit(result.state);
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

    // Replayed prefixes can contain more paths than the bounded wave. Admit the freshly selected
    // transactions first, then use any remaining capacity for downstream replay effects.
    fresh_targets.extend(replayed_targets);
    Some(fresh_targets)
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
    use alloy_primitives::U256;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_evm_ethereum::MockEvmConfig;
    use revm::state::{Account, EvmStorageSlot, TransactionId};
    use std::sync::{atomic::AtomicUsize, mpsc};

    fn noop_calculator() -> Arc<TxPoolTrieProofCalculator> {
        Arc::new(|_| Ok(0))
    }

    fn request(
        parent_hash: B256,
        epoch: u64,
        targets: MultiProofTargetsV2,
        calculator: Arc<TxPoolTrieProofCalculator>,
    ) -> TxPoolTriePrewarmRequest {
        TxPoolTriePrewarmRequest { parent_hash, head_epoch: epoch, epoch, targets, calculator }
    }

    #[test]
    fn changed_state_collects_only_relevant_targets() {
        let address = Address::with_last_byte(1);
        let slot = U256::from(2);
        let mut account = Account::default();
        account.mark_touch();
        account.storage.insert(
            slot,
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(3), TransactionId::ZERO),
        );
        let changed_address = Address::with_last_byte(2);
        let mut changed_account = Account::default();
        changed_account.mark_touch();
        changed_account.info.nonce = 1;
        let destroyed_address = Address::with_last_byte(3);
        let mut destroyed_account = Account::default();
        destroyed_account.mark_touch();
        destroyed_account.mark_selfdestruct();
        let untouched_address = Address::with_last_byte(4);
        let mut untouched_account = Account::default();
        untouched_account.info.nonce = 1;
        let unchanged_address = Address::with_last_byte(5);
        let mut unchanged_account = Account::default();
        unchanged_account.mark_touch();
        let state = EvmState::from_iter([
            (address, account),
            (changed_address, changed_account),
            (destroyed_address, destroyed_account),
            (untouched_address, untouched_account),
            (unchanged_address, unchanged_account),
        ]);

        let mut targets = TxPoolTriePrewarmTargets::default();
        targets.extend_state(&state);
        let targets = targets.into_multiproof_targets();
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(B256::new(slot.to_be_bytes()));

        let account_targets =
            targets.account_targets.iter().map(ProofV2Target::key).collect::<Vec<_>>();
        assert_eq!(account_targets.len(), 3);
        assert!(account_targets.contains(&hashed_address));
        assert!(account_targets.contains(&keccak256(changed_address)));
        assert!(account_targets.contains(&keccak256(destroyed_address)));
        assert!(!account_targets.contains(&keccak256(untouched_address)));
        assert!(!account_targets.contains(&keccak256(unchanged_address)));
        assert_eq!(targets.storage_targets[&hashed_address].len(), 1);
        assert_eq!(targets.storage_targets[&hashed_address][0].key(), hashed_slot);
    }

    #[test]
    fn changed_target_wave_is_bounded() {
        let account = B256::with_last_byte(1);
        let mut targets = TxPoolTriePrewarmTargets::default();
        for slot in 0..TXPOOL_TRIE_PREWARM_MAX_TARGETS_PER_WAVE * 2 {
            targets.insert_storage(account, B256::from(U256::from(slot)));
        }

        assert_eq!(targets.len(), TXPOOL_TRIE_PREWARM_MAX_TARGETS_PER_WAVE);
        assert_eq!(targets.accounts.len(), 1);
        assert_eq!(targets.storages[&account].len(), TXPOOL_TRIE_PREWARM_MAX_TARGETS_PER_WAVE - 1);
    }

    #[test]
    fn chunks_limit_accounts_and_total_targets() {
        let first = B256::with_last_byte(1);
        let second = B256::with_last_byte(2);
        let storage_only = B256::with_last_byte(3);
        let targets = MultiProofTargetsV2 {
            account_targets: vec![ProofV2Target::new(first), ProofV2Target::new(second)],
            storage_targets: B256Map::from_iter([
                (first, (10..19).map(B256::with_last_byte).map(ProofV2Target::new).collect()),
                (
                    storage_only,
                    (20..22).map(B256::with_last_byte).map(ProofV2Target::new).collect(),
                ),
            ]),
        };

        let chunks = txpool_trie_prewarm_chunks(targets, 5);

        assert!(chunks.iter().all(|chunk| chunk.account_targets.len() <= 1));
        assert!(chunks.iter().all(|chunk| chunk.chunking_length() <= 5));
        assert_eq!(chunks.iter().map(MultiProofTargetsV2::chunking_length).sum::<usize>(), 14);
        assert_eq!(
            chunks
                .iter()
                .flat_map(|chunk| &chunk.account_targets)
                .filter(|target| target.key() == storage_only)
                .count(),
            1
        );

        let one_target_chunks = txpool_trie_prewarm_chunks(
            MultiProofTargetsV2 {
                account_targets: vec![ProofV2Target::new(first)],
                storage_targets: B256Map::from_iter([(
                    first,
                    vec![ProofV2Target::new(B256::with_last_byte(10))],
                )]),
            },
            1,
        );
        assert_eq!(one_target_chunks.len(), 2);
        assert!(one_target_chunks.iter().all(|chunk| chunk.chunking_length() == 1));
    }

    #[test]
    fn trie_queue_preserves_oldest_wave_and_rejects_stale_epochs() {
        let requests = TxPoolPrewarmRequests::new();
        let calculator = noop_calculator();

        assert!(requests.try_enqueue(request(
            B256::with_last_byte(1),
            1,
            MultiProofTargetsV2::default(),
            Arc::clone(&calculator),
        )));
        assert!(!requests.try_enqueue(request(
            B256::with_last_byte(2),
            1,
            MultiProofTargetsV2::default(),
            Arc::clone(&calculator),
        )));
        assert_eq!(requests.take().unwrap().parent_hash, B256::with_last_byte(1));

        assert!(requests.try_enqueue(request(
            B256::with_last_byte(3),
            1,
            MultiProofTargetsV2::default(),
            Arc::clone(&calculator),
        )));
        assert!(requests.try_enqueue(request(
            B256::with_last_byte(4),
            2,
            MultiProofTargetsV2::default(),
            calculator,
        )));
        let pending = requests.take().unwrap();
        assert_eq!(pending.parent_hash, B256::with_last_byte(4));
        assert_eq!(pending.epoch, 2);

        requests.advance_epoch(3);
        assert!(!requests.try_enqueue(request(
            B256::with_last_byte(5),
            2,
            MultiProofTargetsV2::default(),
            noop_calculator(),
        )));
    }

    #[test]
    fn repeated_canonical_head_keeps_trie_epoch() {
        let trie = TxPoolTriePrewarmState::new();
        let first = B256::with_last_byte(1);

        assert_eq!(trie.begin_head(first), 1);
        assert_eq!(trie.begin_head(first), 1);
        assert_eq!(trie.begin_head(B256::with_last_byte(2)), 2);
    }

    #[test]
    fn cancellation_waits_for_active_chunk_and_stops_following_chunks() {
        let trie = Arc::new(TxPoolTriePrewarmState::new());
        trie.begin_head(B256::with_last_byte(1));
        let handle = Arc::new(TxPoolPrewarmHandle::<EthPrimitives, (), MockEvmConfig> {
            requests: Arc::new(TxPoolPrewarmRequests::new()),
            trie: Arc::clone(&trie),
            publication: Arc::new(TxPoolSnapshotPublication::default()),
            activity: Arc::new(TxPoolWorkerActivity::default()),
            wait_generation: AtomicU64::new(0),
            _marker: PhantomData,
        });
        let epoch = trie.epoch.load(Ordering::Acquire);
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let calculator: Arc<TxPoolTrieProofCalculator> = {
            let calls = Arc::clone(&calls);
            let release_rx = Arc::clone(&release_rx);
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(0)
            })
        };
        let account = B256::with_last_byte(1);
        let targets = MultiProofTargetsV2 {
            account_targets: vec![ProofV2Target::new(account)],
            storage_targets: B256Map::from_iter([(
                account,
                (2..12).map(B256::with_last_byte).map(ProofV2Target::new).collect(),
            )]),
        };

        let worker = {
            let trie = Arc::clone(&trie);
            std::thread::spawn(move || txpool_trie_prewarm_loop(trie, 5))
        };
        assert!(trie.requests.try_enqueue(request(
            B256::with_last_byte(1),
            epoch,
            targets,
            calculator
        )));
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let (wait_started_tx, wait_started_rx) = mpsc::channel();
        let (wait_done_tx, wait_done_rx) = mpsc::channel();
        let idle_waiter = {
            let handle = Arc::clone(&handle);
            std::thread::spawn(move || {
                wait_started_tx.send(()).unwrap();
                handle.cancel_and_wait();
                wait_done_tx.send(()).unwrap();
            })
        };
        wait_started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(wait_done_rx.try_recv().is_err());

        release_tx.send(()).unwrap();
        wait_done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        idle_waiter.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        trie.requests.close();
        worker.join().unwrap();
    }
}
