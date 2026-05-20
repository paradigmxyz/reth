//! revmc JIT compiler integration for EVM execution (requires the `jit` feature).
//!
//! Re-exports types from `revmc::alloy_evm` and provides [`RethEvmFactory`], a newtype that
//! implements [`Debug`].

#[cfg(feature = "jit")]
use alloc::string::String;
use alloy_evm::{Database, EvmEnv, EvmFactory};
use revm::{
    context::BlockEnv,
    context_interface::result::{EVMError, HaltReason},
    inspector::NoOpInspector,
    primitives::hardfork::SpecId,
    Inspector,
};
#[cfg(feature = "jit")]
use revmc::alloy_evm::JitEvmFactory;

#[cfg(feature = "jit")]
pub use revmc::{
    runtime::{
        maybe_run_jit_helper, CompilationEvent, CompilationKind, JitBackend, JitMode,
        RuntimeConfig, RuntimeStatsSnapshot, RuntimeTuning,
    },
    CompileTimings,
};

#[cfg(feature = "jit")]
type Inner = JitEvmFactory;
#[cfg(not(feature = "jit"))]
type Inner = alloy_evm::EthEvmFactory;

/// Reth EVM factory.
///
/// With the `jit` feature, this wraps [`JitEvmFactory`] and owns the shared revmc backend. The
/// backend is constructed for every node so runtime RPC controls can enable it later.
///
/// An EVM can execute JIT-compiled code only when all three gates are enabled: the binary was built
/// with the `jit` feature, runtime compilation was enabled with `--jit` or the `reth_jit` RPC
/// method, and the local EVM config selected JIT support with
/// [`ConfigureEvm::with_jit_support`](reth_evm::ConfigureEvm::with_jit_support).
///
/// Without the `jit` feature, this is a thin wrapper around [`alloy_evm::EthEvmFactory`].
#[derive(Debug)]
pub struct RethEvmFactory {
    inner: Inner,
    #[cfg(feature = "jit")]
    disabled: JitBackend,
    #[cfg(feature = "jit")]
    metrics: RevmcMetrics,
    #[cfg(feature = "jit")]
    jit_support: bool,
}

impl Clone for RethEvmFactory {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            #[cfg(feature = "jit")]
            disabled: self.disabled.clone(),
            #[cfg(feature = "jit")]
            metrics: self.metrics.clone(),
            #[cfg(feature = "jit")]
            jit_support: self.jit_support,
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for RethEvmFactory {
    fn default() -> Self {
        #[cfg(feature = "jit")]
        {
            Self::new(JitBackend::disabled())
        }
        #[cfg(not(feature = "jit"))]
        {
            Self { inner: Default::default() }
        }
    }
}

#[cfg(feature = "jit")]
impl RethEvmFactory {
    /// Creates a new factory that owns the backend.
    pub fn new(backend: JitBackend) -> Self {
        Self::new_with_metrics(backend, RevmcMetrics::default())
    }

    /// Creates a new factory that owns the backend and records metrics into the given handles.
    pub fn new_with_metrics(backend: JitBackend, metrics: RevmcMetrics) -> Self {
        Self {
            inner: JitEvmFactory::new(backend),
            disabled: JitBackend::disabled(),
            metrics,
            jit_support: false,
        }
    }

    /// Creates a [`RethEvmFactory`] with JIT support and compilation disabled.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Returns a reference to the JIT backend.
    pub const fn backend(&self) -> &JitBackend {
        self.inner.backend()
    }

    /// Enables or disables local JIT support for subsequently created EVMs.
    ///
    /// Enabling support only selects the JIT-capable EVM factory path. An EVM still requires the
    /// `jit` feature and runtime compilation enabled by `--jit` or `reth_jit` before it can execute
    /// JIT-compiled code.
    pub const fn set_jit_support(&mut self, enabled: bool) {
        self.jit_support = enabled;
    }

    /// Returns whether subsequently created EVMs use the JIT-capable factory path.
    pub const fn jit_support_enabled(&self) -> bool {
        self.jit_support
    }

    /// Pauses JIT helper execution while keeping queueing and resident lookups enabled.
    fn pause_jit(&self) {
        let backend = self.inner.backend();
        let was_paused = backend.is_paused();
        backend.pause();
        let is_paused = backend.is_paused();
        if !was_paused && is_paused {
            self.metrics.pauses_total.increment(1);
        }
        self.metrics.paused.set(is_paused as u8 as f64);
    }

    /// Resumes background JIT promotion.
    fn resume_jit(&self) {
        let backend = self.inner.backend();
        let was_paused = backend.is_paused();
        backend.resume();
        let is_paused = backend.is_paused();
        if was_paused && !is_paused {
            self.metrics.resumes_total.increment(1);
        }
        self.metrics.paused.set(is_paused as u8 as f64);
    }
}

#[cfg(feature = "jit")]
impl reth_evm::JitBackend for RethEvmFactory {
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.inner.backend().set_enabled(enabled).map_err(|err| err.to_string())
    }

    fn pause(&self) {
        self.pause_jit();
    }

    fn resume(&self) {
        self.resume_jit();
    }

    fn clear(&self) {
        self.inner.backend().clear_all();
    }
}

impl EvmFactory for RethEvmFactory {
    type Evm<DB: Database, I: Inspector<alloy_evm::eth::EthEvmContext<DB>>> =
        <Inner as EvmFactory>::Evm<DB, I>;
    type Context<DB: Database> = <Inner as EvmFactory>::Context<DB>;
    type Tx = <Inner as EvmFactory>::Tx;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = <Inner as EvmFactory>::Precompiles;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        #[cfg(feature = "jit")]
        {
            if self.jit_support {
                self.inner.create_evm(db, input)
            } else {
                JitEvmFactory::new(self.disabled.clone()).create_evm(db, input)
            }
        }
        #[cfg(not(feature = "jit"))]
        {
            self.inner.create_evm(db, input)
        }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        #[cfg(feature = "jit")]
        {
            if self.jit_support {
                self.inner.create_evm_with_inspector(db, input, inspector)
            } else {
                JitEvmFactory::new(self.disabled.clone())
                    .create_evm_with_inspector(db, input, inspector)
            }
        }
        #[cfg(not(feature = "jit"))]
        {
            self.inner.create_evm_with_inspector(db, input, inspector)
        }
    }
}

/// Prometheus metrics for revmc JIT runtime stats.
#[cfg(feature = "jit")]
#[derive(reth_metrics::Metrics, Clone)]
#[metrics(scope = "revmc.jit")]
pub struct RevmcMetrics {
    /// Total lookups that returned a compiled function.
    pub lookup_hits: metrics::Gauge,
    /// Total lookups that returned interpret (not ready).
    pub lookup_misses: metrics::Gauge,
    /// Lookup-observed events currently queued.
    pub events_queued: metrics::Gauge,
    /// Lookup-observed events dropped (channel full).
    pub events_dropped: metrics::Gauge,
    /// Number of entries in the resident compiled map.
    pub resident_entries: metrics::Gauge,
    /// Approximate total bytes of compiled machine code in the resident map.
    pub jit_code_bytes: metrics::Gauge,
    /// Approximate total bytes of JIT-related data (relocations, metadata, etc.).
    pub jit_data_bytes: metrics::Gauge,
    /// Number of pending control commands queued for the backend.
    pub command_queue_len: metrics::Gauge,
    /// Number of compilation jobs dispatched but not completed yet.
    pub pending_jobs: metrics::Gauge,
    /// Total number of entries evicted (idle + budget).
    pub evictions: metrics::Gauge,
    /// Total number of compilations dispatched (JIT promotions + AOT requests).
    pub compilations_dispatched: metrics::Gauge,
    /// Total number of successful compilations (JIT + AOT).
    pub compilations_succeeded: metrics::Gauge,
    /// Total number of failed compilations (JIT + AOT).
    pub compilations_failed: metrics::Gauge,
    /// Total number of JIT helper processes spawned.
    pub jit_helper_spawns: metrics::Gauge,
    /// Total number of JIT helper process spawn failures.
    pub jit_helper_spawn_failures: metrics::Gauge,
    /// Total number of JIT helper process restarts.
    pub jit_helper_restarts: metrics::Gauge,
    /// Total number of JIT helper job timeouts.
    pub jit_helper_timeouts: metrics::Gauge,
    /// Total number of JIT helper process disconnects.
    pub jit_helper_disconnects: metrics::Gauge,
    /// Total number of transitions into paused JIT helper execution.
    pub pauses_total: metrics::Counter,
    /// Total number of transitions out of paused JIT helper execution.
    pub resumes_total: metrics::Counter,
    /// Whether JIT helper execution is currently paused.
    pub paused: metrics::Gauge,
    /// Histogram of total JIT compilation durations (seconds).
    pub jit_compilation_duration: metrics::Histogram,
    /// Duration of the last JIT compilation (seconds).
    pub jit_compilation_duration_last: metrics::Gauge,
    /// Histogram of parse phase durations (seconds).
    pub jit_parse_duration: metrics::Histogram,
    /// Histogram of translate phase durations (seconds).
    pub jit_translate_duration: metrics::Histogram,
    /// Histogram of optimize phase durations (seconds).
    pub jit_optimize_duration: metrics::Histogram,
    /// Histogram of codegen phase durations (seconds).
    pub jit_codegen_duration: metrics::Histogram,
}

#[cfg(feature = "jit")]
impl RevmcMetrics {
    /// Records a [`RuntimeStatsSnapshot`] into the metrics.
    pub fn record(&self, stats: &RuntimeStatsSnapshot) {
        let RuntimeStatsSnapshot {
            lookup_hits,
            lookup_misses,
            events_dropped,
            resident_entries,
            events_queued,
            command_queue_len,
            pending_jobs,
            jit_code_bytes,
            jit_data_bytes,
            evictions,
            compilations_dispatched,
            compilations_succeeded,
            compilations_failed,
            jit_helper_spawns,
            jit_helper_spawn_failures,
            jit_helper_restarts,
            jit_helper_timeouts,
            jit_helper_disconnects,
            ..
        } = *stats;
        self.lookup_hits.set(lookup_hits as f64);
        self.lookup_misses.set(lookup_misses as f64);
        self.events_queued.set(events_queued as f64);
        self.events_dropped.set(events_dropped as f64);
        self.resident_entries.set(resident_entries as f64);
        self.jit_code_bytes.set(jit_code_bytes as f64);
        self.jit_data_bytes.set(jit_data_bytes as f64);
        self.command_queue_len.set(command_queue_len as f64);
        self.pending_jobs.set(pending_jobs as f64);
        self.evictions.set(evictions as f64);
        self.compilations_dispatched.set(compilations_dispatched as f64);
        self.compilations_succeeded.set(compilations_succeeded as f64);
        self.compilations_failed.set(compilations_failed as f64);
        self.jit_helper_spawns.set(jit_helper_spawns as f64);
        self.jit_helper_spawn_failures.set(jit_helper_spawn_failures as f64);
        self.jit_helper_restarts.set(jit_helper_restarts as f64);
        self.jit_helper_timeouts.set(jit_helper_timeouts as f64);
        self.jit_helper_disconnects.set(jit_helper_disconnects as f64);
    }

    /// Records a [`CompilationEvent`] into the histogram metrics.
    pub fn record_compilation(&self, event: &CompilationEvent) {
        let duration_secs = event.duration.as_secs_f64();
        self.jit_compilation_duration.record(duration_secs);
        self.jit_compilation_duration_last.set(duration_secs);
        self.jit_parse_duration.record(event.timings.parse.as_secs_f64());
        self.jit_translate_duration.record(event.timings.translate.as_secs_f64());
        self.jit_optimize_duration.record(event.timings.optimize.as_secs_f64());
        self.jit_codegen_duration.record(event.timings.codegen.as_secs_f64());
    }
}
