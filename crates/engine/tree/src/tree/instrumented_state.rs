//! Implements a state provider that tracks latency metrics.
use alloy_primitives::{Address, StorageKey, StorageValue, B256};
use metrics::{Gauge, Histogram};
use reth_errors::ProviderResult;
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProofProvider, StateProvider,
    StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use std::{
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Nanoseconds per second
const NANOS_PER_SEC: u32 = 1_000_000_000;

/// An atomic version of [`Duration`], using an [`AtomicU64`] to store the seconds part of the
/// duration, and an [`AtomicU32`] for the nanoseconds part.
#[derive(Default)]
pub(crate) struct AtomicDuration {
    /// The seconds part of the duration
    seconds: AtomicU64,

    /// The nanoseconds part of the duration
    nanos: AtomicU32,
}

impl AtomicDuration {
    /// The zero duration.
    const ZERO: Self = Self { seconds: AtomicU64::new(0), nanos: AtomicU32::new(0) };

    /// Creates a new [`AtomicDuration`] from a [`Duration`]
    pub(crate) fn new(duration: Duration) -> Self {
        Self {
            seconds: AtomicU64::new(duration.as_secs()),
            nanos: AtomicU32::new(duration.subsec_nanos()),
        }
    }

    /// Returns the duration as a [`Duration`]
    pub(crate) fn duration(&self) -> Duration {
        Duration::new(self.seconds.load(Ordering::Relaxed), self.nanos.load(Ordering::Relaxed))
    }

    /// Adds a [`Duration`] to the atomic duration.
    pub(crate) fn add_duration(&self, duration: Duration) {
        // add the seconds part of the duration
        self.seconds.fetch_add(duration.as_secs(), Ordering::Relaxed);
        // add the nanoseconds part of the duration
        self.nanos.fetch_add(duration.subsec_nanos(), Ordering::Relaxed);

        // carry over the nanoseconds to the seconds part if the nanoseconds part is greater than 1
        // second
        if self.nanos.load(Ordering::Relaxed) >= NANOS_PER_SEC {
            // get the number of seconds to carry over
            let carry_over = self.nanos.load(Ordering::Relaxed) / NANOS_PER_SEC;
            // set the nanoseconds part to the remainder
            self.nanos.fetch_sub(carry_over * NANOS_PER_SEC, Ordering::Relaxed);
            // add the carry over to the seconds part
            self.seconds.fetch_add(carry_over as u64, Ordering::Relaxed);
        }
    }
}

/// A wrapper of a state provider and latency metrics.
pub(crate) struct InstrumentedStateProvider<S> {
    /// The state provider
    state_provider: S,

    /// Metrics for the instrumented state provider
    metrics: StateProviderMetrics,

    /// The total time we spend fetching storage over the lifetime of this state provider
    total_storage_fetch_latency: AtomicDuration,

    /// The total time we spend fetching code over the lifetime of this state provider
    total_code_fetch_latency: AtomicDuration,

    /// The total time we spend fetching accounts over the lifetime of this state provider
    total_account_fetch_latency: AtomicDuration,
}

impl<S> InstrumentedStateProvider<S>
where
    S: StateProvider,
{
    /// Creates a new [`InstrumentedStateProvider`] from a state provider
    pub(crate) fn from_state_provider(state_provider: S) -> Self {
        Self {
            state_provider,
            metrics: StateProviderMetrics::default(),
            total_storage_fetch_latency: AtomicDuration::ZERO,
            total_code_fetch_latency: AtomicDuration::ZERO,
            total_account_fetch_latency: AtomicDuration::ZERO,
        }
    }
}

impl<S> InstrumentedStateProvider<S> {
    /// Records the latency for a storage fetch, and increments the duration counter for the storage
    /// fetch.
    fn record_storage_fetch(&self, latency: Duration) {
        self.metrics.storage_fetch_latency.record(latency);
        self.total_storage_fetch_latency.add_duration(latency);
    }

    /// Records the latency for a code fetch, and increments the duration counter for the code
    /// fetch.
    fn record_code_fetch(&self, latency: Duration) {
        self.metrics.code_fetch_latency.record(latency);
        self.total_code_fetch_latency.add_duration(latency);
    }

    /// Records the latency for an account fetch, and increments the duration counter for the
    /// account fetch.
    fn record_account_fetch(&self, latency: Duration) {
        self.metrics.account_fetch_latency.record(latency);
        self.total_account_fetch_latency.add_duration(latency);
    }

    /// Records the total latencies into their respective gauges and histograms.
    fn record_total_latency(&self) {
        let total_storage_fetch_latency = self.total_storage_fetch_latency.duration();
        self.metrics.total_storage_fetch_latency.record(total_storage_fetch_latency);
        self.metrics
            .total_storage_fetch_latency_gauge
            .set(total_storage_fetch_latency.as_secs_f64());

        let total_code_fetch_latency = self.total_code_fetch_latency.duration();
        self.metrics.total_code_fetch_latency.record(total_code_fetch_latency);
        self.metrics.total_code_fetch_latency_gauge.set(total_code_fetch_latency.as_secs_f64());

        let total_account_fetch_latency = self.total_account_fetch_latency.duration();
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
        self.record_account_fetch(start.elapsed());
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
        self.record_storage_fetch(start.elapsed());
        res
    }

    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        let start = Instant::now();
        let res = self.state_provider.bytecode_by_hash(code_hash);
        self.record_code_fetch(start.elapsed());
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
    ) -> ProviderResult<Vec<alloy_primitives::Bytes>> {
        self.state_provider.witness(input, target)
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
