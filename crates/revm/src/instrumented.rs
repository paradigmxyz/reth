use crate::database::EvmStateProvider;
use alloy_primitives::{Address, B256, U256};
use core::ops::{Deref, DerefMut};
use reth_metrics::{
    metrics::{Gauge, Histogram},
    Metrics,
};

use reth_storage_errors::provider::ProviderError;
use revm::{bytecode::Bytecode, state::AccountInfo, Database, DatabaseRef};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
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
    /// Returns a zero duration.
    pub(crate) const fn zero() -> Self {
        Self { nanos: AtomicU64::new(0) }
    }

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

/// A [Database] and [`DatabaseRef`] implementation that uses [`EvmStateProvider`] as the underlying
/// data source.
pub struct InstrumentedStateProviderDatabase<DB> {
    /// Database
    db: DB,

    /// Metrics for the instrumented state provider database
    metrics: InstrumentedStateProviderDatabaseMetrics,

    /// The total time we spend fetching storage over the lifetime of this state provider
    total_storage_fetch_latency: AtomicDuration,

    /// The total time we spend fetching code over the lifetime of this state provider
    total_code_fetch_latency: AtomicDuration,

    /// The total time we spend fetching accounts over the lifetime of this state provider
    total_account_fetch_latency: AtomicDuration,
}

impl<DB> InstrumentedStateProviderDatabase<DB> {
    /// Create new State with generic `StateProvider`.
    pub fn new(db: DB, source: &'static str) -> Self {
        Self {
            db,
            metrics: InstrumentedStateProviderDatabaseMetrics::new_with_labels(&[(
                "source", source,
            )]),
            total_storage_fetch_latency: AtomicDuration::zero(),
            total_code_fetch_latency: AtomicDuration::zero(),
            total_account_fetch_latency: AtomicDuration::zero(),
        }
    }

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
    pub(crate) fn record_total_latency(&self) {
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

impl<DB> core::fmt::Debug for InstrumentedStateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InstrumentedStateProviderDatabase").finish_non_exhaustive()
    }
}

impl<DB> AsRef<DB> for InstrumentedStateProviderDatabase<DB> {
    fn as_ref(&self) -> &DB {
        self
    }
}

impl<DB> Deref for InstrumentedStateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl<DB> DerefMut for InstrumentedStateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl<DB> Drop for InstrumentedStateProviderDatabase<DB> {
    fn drop(&mut self) {
        self.record_total_latency();
    }
}

impl<DB: EvmStateProvider> Database for InstrumentedStateProviderDatabase<DB> {
    type Error = ProviderError;

    /// Retrieves basic account information for a given address.
    ///
    /// Returns `Ok` with `Some(AccountInfo)` if the account exists,
    /// `None` if it doesn't, or an error if encountered.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.basic_ref(address)
    }

    /// Retrieves the bytecode associated with a given code hash.
    ///
    /// Returns `Ok` with the bytecode if found, or the default bytecode otherwise.
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash_ref(code_hash)
    }

    /// Retrieves the storage value at a specific index for a given address.
    ///
    /// Returns `Ok` with the storage value, or the default value if not found.
    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage_ref(address, index)
    }

    /// Retrieves the block hash for a given block number.
    ///
    /// Returns `Ok` with the block hash if found, or the default hash otherwise.
    /// Note: It safely casts the `number` to `u64`.
    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.block_hash_ref(number)
    }
}

impl<DB: EvmStateProvider> DatabaseRef for InstrumentedStateProviderDatabase<DB> {
    type Error = <Self as Database>::Error;

    /// Retrieves basic account information for a given address.
    ///
    /// Returns `Ok` with `Some(AccountInfo)` if the account exists,
    /// `None` if it doesn't, or an error if encountered.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let start = Instant::now();
        let res = self.basic_account(&address);
        self.record_account_fetch(start.elapsed());
        Ok(res?.map(Into::into))
    }

    /// Retrieves the bytecode associated with a given code hash.
    ///
    /// Returns `Ok` with the bytecode if found, or the default bytecode otherwise.
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let start = Instant::now();
        let res = self.bytecode_by_hash(&code_hash);
        self.record_code_fetch(start.elapsed());
        Ok(res?.unwrap_or_default().0)
    }

    /// Retrieves the storage value at a specific index for a given address.
    ///
    /// Returns `Ok` with the storage value, or the default value if not found.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let start = Instant::now();
        let res = self.db.storage(address, B256::new(index.to_be_bytes()));
        self.record_storage_fetch(start.elapsed());
        Ok(res?.unwrap_or_default())
    }

    /// Retrieves the block hash for a given block number.
    ///
    /// Returns `Ok` with the block hash if found, or the default hash otherwise.
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        // Get the block hash or default hash with an attempt to convert U256 block number to u64
        Ok(self.db.block_hash(number)?.unwrap_or_default())
    }
}

/// Metrics for the instrumented state provider database
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.state_provider_database")]
pub(crate) struct InstrumentedStateProviderDatabaseMetrics {
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
