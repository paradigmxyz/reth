//! Implements a state provider that tracks latency metrics.
use alloy_primitives::{Address, StorageKey, StorageValue, B256};
use metrics::{Gauge, Histogram};
use reth_errors::ProviderResult;
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, Bytecode, FastInstant as Instant};
use reth_provider::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use std::{
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

/// Nanoseconds per second
const NANOS_PER_SEC: u32 = 1_000_000_000;

/// An atomic version of [`Duration`], using an [`AtomicU64`] to store the total nanoseconds in the
/// duration.
#[derive(Debug, Default)]
pub(crate) struct AtomicDuration {
    /// The nanoseconds part of the duration
    ///
    /// We would have to accumulate 584 years of nanoseconds to overflow a u64, so this is
    /// sufficiently large for our use case. We don't expect to be adding arbitrary durations to
    /// this value.
    nanos: AtomicU64,
}

impl AtomicDuration {
    /// Returns the duration as a [`Duration`]
    pub(crate) fn duration(&self) -> Duration {
        let nanos = self.nanos.load(Ordering::Relaxed);
        let seconds = nanos / NANOS_PER_SEC as u64;
        let nanos = nanos % NANOS_PER_SEC as u64;
        // `as u32` is ok because we did a mod by u32 const
        Duration::new(seconds, nanos as u32)
    }

    /// Adds a [`Duration`] to the atomic duration.
    pub(crate) fn add_duration(&self, duration: Duration) {
        // this is `as_nanos` but without the `as u128` - we do not expect durations over 584 years
        // as input here
        let total_nanos =
            duration.as_secs() * NANOS_PER_SEC as u64 + duration.subsec_nanos() as u64;
        // add the nanoseconds part of the duration
        self.nanos.fetch_add(total_nanos, Ordering::Relaxed);
    }
}

/// A wrapper of a state provider and latency metrics.
#[derive(Debug)]
pub struct InstrumentedStateProvider<S> {
    /// The state provider
    state_provider: S,
    /// Prometheus metrics for the instrumented state provider
    metrics: StateProviderMetrics,
    /// Shared fetch statistics, readable after the provider is consumed.
    stats: Arc<StateProviderStats>,
}

impl<S> InstrumentedStateProvider<S>
where
    S: StateProvider,
{
    /// Creates a new [`InstrumentedStateProvider`] from a state provider with the provided label
    /// for metrics.
    pub fn new(state_provider: S, source: &'static str) -> Self {
        Self {
            state_provider,
            metrics: StateProviderMetrics::new_with_labels(&[("source", source)]),
            stats: Arc::new(StateProviderStats::default()),
        }
    }

    /// Returns a shared reference to the accumulated fetch statistics.
    pub fn stats(&self) -> Arc<StateProviderStats> {
        Arc::clone(&self.stats)
    }
}

impl<S> Drop for InstrumentedStateProvider<S> {
    fn drop(&mut self) {
        let total_storage_fetch_latency = self.stats.total_storage_fetch_latency.duration();
        self.metrics.total_storage_fetch_latency.record(total_storage_fetch_latency);
        self.metrics
            .total_storage_fetch_latency_gauge
            .set(total_storage_fetch_latency.as_secs_f64());

        let total_code_fetch_latency = self.stats.total_code_fetch_latency.duration();
        self.metrics.total_code_fetch_latency.record(total_code_fetch_latency);
        self.metrics.total_code_fetch_latency_gauge.set(total_code_fetch_latency.as_secs_f64());

        let total_account_fetch_latency = self.stats.total_account_fetch_latency.duration();
        self.metrics.total_account_fetch_latency.record(total_account_fetch_latency);
        self.metrics
            .total_account_fetch_latency_gauge
            .set(total_account_fetch_latency.as_secs_f64());
    }
}

/// Metrics for the instrumented state provider
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.state_provider")]
pub(crate) struct StateProviderMetrics {
    /// A histogram of the time it takes to get a storage value
    storage_fetch_latency: Histogram,

    /// A histogram of the time it takes to get a code value
    code_fetch_latency: Histogram,

    /// A histogram of the time it takes to get an account value
    account_fetch_latency: Histogram,

    /// A histogram of the total time we spend fetching storage over the lifetime of this state
    /// provider
    total_storage_fetch_latency: Histogram,

    /// A gauge of the total time we spend fetching storage over the lifetime of this state
    /// provider
    total_storage_fetch_latency_gauge: Gauge,

    /// A histogram of the total time we spend fetching code over the lifetime of this state
    /// provider
    total_code_fetch_latency: Histogram,

    /// A gauge of the total time we spend fetching code over the lifetime of this state provider
    total_code_fetch_latency_gauge: Gauge,

    /// A histogram of the total time we spend fetching accounts over the lifetime of this state
    /// provider
    total_account_fetch_latency: Histogram,

    /// A gauge of the total time we spend fetching accounts over the lifetime of this state
    /// provider
    total_account_fetch_latency_gauge: Gauge,
}

impl<S: AccountReader> AccountReader for InstrumentedStateProvider<S> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        let start = Instant::now();
        let res = self.state_provider.basic_account(address);
        let elapsed = start.elapsed();
        self.metrics.account_fetch_latency.record(elapsed);
        self.stats.total_account_fetches.fetch_add(1, Ordering::Relaxed);
        self.stats.total_account_fetch_latency.add_duration(elapsed);
        res
    }
}

impl<S: StateProvider> StateProvider for InstrumentedStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let start = Instant::now();
        let res = self.state_provider.storage(account, storage_key);
        let elapsed = start.elapsed();
        self.metrics.storage_fetch_latency.record(elapsed);
        self.stats.total_storage_fetches.fetch_add(1, Ordering::Relaxed);
        self.stats.total_storage_fetch_latency.add_duration(elapsed);
        res
    }
}

impl<S: BytecodeReader> BytecodeReader for InstrumentedStateProvider<S> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        let start = Instant::now();
        let res = self.state_provider.bytecode_by_hash(code_hash);
        let elapsed = start.elapsed();
        self.metrics.code_fetch_latency.record(elapsed);
        self.stats.total_code_fetches.fetch_add(1, Ordering::Relaxed);
        self.stats.total_code_fetch_latency.add_duration(elapsed);
        self.stats.total_code_fetched_bytes.fetch_add(
            res.as_ref()
                .ok()
                .and_then(|code| code.as_ref().map(|code| code.len()))
                .unwrap_or_default(),
            Ordering::Relaxed,
        );
        res
    }
}

impl<S: StateRootProvider> StateRootProvider for InstrumentedStateProvider<S> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.state_provider.state_root(hashed_state)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.state_provider.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_provider.state_root_with_updates(hashed_state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_provider.state_root_from_nodes_with_updates(input)
    }
}

impl<S: StateProofProvider> StateProofProvider for InstrumentedStateProvider<S> {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        self.state_provider.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        self.state_provider.multiproof(input, targets)
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: reth_trie::ExecutionWitnessMode,
    ) -> ProviderResult<Vec<alloy_primitives::Bytes>> {
        self.state_provider.witness(input, target, mode)
    }
}

impl<S: StorageRootProvider> StorageRootProvider for InstrumentedStateProvider<S> {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.state_provider.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.state_provider.storage_proof(address, slot, hashed_storage)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.state_provider.storage_multiproof(address, slots, hashed_storage)
    }
}

impl<S: BlockHashReader> BlockHashReader for InstrumentedStateProvider<S> {
    fn block_hash(&self, number: alloy_primitives::BlockNumber) -> ProviderResult<Option<B256>> {
        self.state_provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: alloy_primitives::BlockNumber,
        end: alloy_primitives::BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.state_provider.canonical_hashes_range(start, end)
    }
}

impl<S: HashedPostStateProvider> HashedPostStateProvider for InstrumentedStateProvider<S> {
    fn hashed_post_state(&self, bundle_state: &reth_revm::db::BundleState) -> HashedPostState {
        self.state_provider.hashed_post_state(bundle_state)
    }
}

/// Accumulated fetch statistics from an [`InstrumentedStateProvider`].
///
/// Shared via `Arc` so statistics can be read after the provider is consumed.
#[derive(Debug, Default)]
pub struct StateProviderStats {
    total_storage_fetches: AtomicUsize,
    total_storage_fetch_latency: AtomicDuration,

    total_code_fetches: AtomicUsize,
    total_code_fetch_latency: AtomicDuration,
    total_code_fetched_bytes: AtomicUsize,

    total_account_fetches: AtomicUsize,
    total_account_fetch_latency: AtomicDuration,
}

impl StateProviderStats {
    /// Returns total number of storage fetches.
    pub fn total_storage_fetches(&self) -> usize {
        self.total_storage_fetches.load(Ordering::Relaxed)
    }

    /// Returns total time spent on storage fetches.
    pub fn total_storage_fetch_latency(&self) -> Duration {
        self.total_storage_fetch_latency.duration()
    }

    /// Returns total number of code fetches.
    pub fn total_code_fetches(&self) -> usize {
        self.total_code_fetches.load(Ordering::Relaxed)
    }

    /// Returns total time spent on code fetches.
    pub fn total_code_fetch_latency(&self) -> Duration {
        self.total_code_fetch_latency.duration()
    }

    /// Returns total amount of code fetched, in bytes.
    pub fn total_code_fetched_bytes(&self) -> usize {
        self.total_code_fetched_bytes.load(Ordering::Relaxed)
    }

    /// Returns total number of account fetches.
    pub fn total_account_fetches(&self) -> usize {
        self.total_account_fetches.load(Ordering::Relaxed)
    }

    /// Returns total time spent on account fetches.
    pub fn total_account_fetch_latency(&self) -> Duration {
        self.total_account_fetch_latency.duration()
    }
}
