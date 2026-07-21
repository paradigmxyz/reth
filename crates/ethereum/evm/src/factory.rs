#[cfg(feature = "jit")]
use alloc::string::String;
use alloc::{boxed::Box, sync::Arc};
use alloy_consensus::Header;
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks};
#[cfg(feature = "std")]
use reth_evm::precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap};

#[cfg(feature = "jit")]
pub use evm2_jit::{
    runtime::{
        maybe_run_jit_helper, CompilationEvent, CompilationKind, JitBackend, JitMode,
        RuntimeConfig, RuntimeStatsSnapshot, RuntimeTuning,
    },
    CompileTimings,
};

use crate::{
    executor::{EthBigBlockExecutor, EthBigBlockPlan, HashedStateMode},
    EthBlockExecutionCtx, EthBlockExecutor, EthEvmEnv,
};
use reth_ethereum_primitives::{Receipt, TransactionSigned};

/// Factory used to construct an evm2-backed EVM.
///
/// The transaction type defaults to Ethereum's envelope so the standard block
/// executor remains ergonomic. Custom-node integrations can select a different evm2 transaction
/// type by implementing `EvmFactory<CustomTx>` and using that factory with their own executor and
/// primitives.
pub trait EvmFactory<Tx = evm2::ethereum::TxEnvelope>:
    Clone + core::fmt::Debug + Send + Sync + Unpin + 'static
where
    <Self::Types as evm2::EvmTypesHost>::BlockEnvExt: Send + Sync,
    <Self::Types as evm2::EvmTypesHost>::EvmExt: Default,
    <Self::Types as evm2::EvmTypesHost>::TxResultExt: Send,
{
    /// Runtime evm2 type family used by the EVM.
    type Types: evm2::EvmTypes<Tx = Tx> + evm2::EvmTypesHost<SpecId = Self::SpecId>;

    /// Runtime specification identifier used by the EVM type family.
    type SpecId: Copy
        + Into<evm2::SpecId>
        + Eq
        + core::hash::Hash
        + core::fmt::Debug
        + Send
        + Sync
        + Clone
        + 'static;

    /// Maps an Ethereum hardfork to the runtime specification selected by this factory.
    fn spec_id(&self, spec: evm2::SpecId) -> Self::SpecId;

    /// Converts the standard Ethereum block environment into the factory's block environment.
    fn block_env(&self, block: evm2::env::BlockEnv) -> evm2::env::BlockEnv<Self::Types> {
        evm2::env::BlockEnv {
            number: block.number,
            beneficiary: block.beneficiary,
            timestamp: block.timestamp,
            gas_limit: block.gas_limit,
            basefee: block.basefee,
            difficulty: block.difficulty,
            prevrandao: block.prevrandao,
            blob_basefee: block.blob_basefee,
            slot_num: block.slot_num,
            ext: Default::default(),
            _non_exhaustive: (),
        }
    }

    /// Returns the runtime version for an Ethereum hardfork.
    fn version(&self, spec: evm2::SpecId, chain_id: u64) -> evm2::Version {
        let mut version = evm2::Version::new(spec);
        version.chain_id = chain_id;
        version
    }

    /// Builds the execution configuration for a runtime specification.
    fn execution_config(
        &self,
        spec: Self::SpecId,
        version: evm2::Version,
    ) -> evm2::ExecutionConfig<Self::Types> {
        evm2::ExecutionConfig::for_spec_and_version(spec, version)
    }

    /// Builds the transaction registry for a runtime specification.
    fn tx_registry(
        &self,
        spec: Self::SpecId,
    ) -> evm2::registry::TxRegistry<Self::Types, evm2::TxResult<Self::Types>>;

    /// Builds the precompile table for a runtime specification.
    fn precompiles(&self, spec: Self::SpecId) -> evm2::Precompiles<Self::Types> {
        evm2::Precompiles::base(spec.into())
    }

    /// Applies factory-specific configuration to a newly created EVM.
    fn configure_evm(&self, _evm: &mut evm2::Evm<'_, Self::Types>) {}
}

impl EvmFactory for () {
    type Types = evm2::BaseEvmTypes;
    type SpecId = evm2::SpecId;

    fn spec_id(&self, spec: evm2::SpecId) -> evm2::SpecId {
        spec
    }

    fn tx_registry(
        &self,
        spec: Self::SpecId,
    ) -> evm2::registry::TxRegistry<Self::Types, evm2::TxResult<Self::Types>> {
        evm2::ethereum::ethereum_tx_registry(spec)
    }
}

/// Ethereum block executor factory.
#[derive(Debug)]
pub struct EthBlockExecutorFactory<C = ChainSpec, F = RethEvmFactory>
where
    F: EvmFactory,
{
    /// Chain specification.
    chain_spec: Arc<C>,
    /// Shared precompile cache.
    #[cfg(feature = "std")]
    precompile_cache_map: PrecompileCacheMap<F::SpecId>,
    /// Whether to disable the shared precompile cache.
    #[cfg(feature = "std")]
    precompile_cache_disabled: bool,
    /// EVM factory configuration.
    evm_factory: F,
}

/// Executor factory for merged payloads that switch block context at segment boundaries.
#[derive(Debug, Clone)]
pub struct EthBigBlockExecutorFactory<C = ChainSpec, F = RethEvmFactory>
where
    F: EvmFactory,
{
    inner: EthBlockExecutorFactory<C, F>,
}

impl<C, F: EvmFactory> EthBigBlockExecutorFactory<C, F> {
    /// Creates a big-block executor factory from the standard Ethereum factory.
    pub const fn new(inner: EthBlockExecutorFactory<C, F>) -> Self {
        Self { inner }
    }

    /// Returns the wrapped Ethereum executor factory.
    pub const fn inner(&self) -> &EthBlockExecutorFactory<C, F> {
        &self.inner
    }
}

impl<C, F> reth_evm::BlockExecutorFactory for EthBigBlockExecutorFactory<C, F>
where
    C: EthChainSpec<Header = Header> + EthereumHardforks,
    F: EvmFactory,
{
    type EvmFactory = F;
    type EvmTypes = F::Types;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm<'a> = evm2::Evm<'a, F::Types>;
    type EvmEnv = EthEvmEnv<F::Types>;
    type ExecutionCtx<'a>
        = EthBigBlockPlan<'a, F::Types>
    where
        Self: 'a;
    type Executor<'a>
        = EthBigBlockExecutor<'a, C, F>
    where
        Self: 'a;

    fn create_executor<'a>(
        &'a self,
        evm: Self::Evm<'a>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a>
    where
        Self: 'a,
    {
        let executor = self.inner.create_eth_executor(evm, ctx.segments[0].ctx.clone());
        EthBigBlockExecutor::new(executor, &self.inner, self.inner.chain_spec().clone(), ctx)
    }

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn evm_with_env<'a, DB>(&self, db: DB, evm_env: Self::EvmEnv) -> Self::Evm<'a>
    where
        DB: evm2::evm::DynDatabase + 'a,
    {
        self.inner.evm_with_env(db, evm_env)
    }
}

impl<C, F: EvmFactory> Clone for EthBlockExecutorFactory<C, F> {
    fn clone(&self) -> Self {
        Self {
            chain_spec: self.chain_spec.clone(),
            #[cfg(feature = "std")]
            precompile_cache_map: self.precompile_cache_map.clone(),
            #[cfg(feature = "std")]
            precompile_cache_disabled: self.precompile_cache_disabled,
            evm_factory: self.evm_factory.clone(),
        }
    }
}

impl<C> EthBlockExecutorFactory<C> {
    /// Creates a new Ethereum block executor factory.
    pub fn new(chain_spec: Arc<C>) -> Self {
        Self::new_with_evm_factory(chain_spec, RethEvmFactory::default())
    }
}

impl<C, F: EvmFactory> EthBlockExecutorFactory<C, F> {
    /// Creates a new Ethereum block executor factory with the given EVM factory configuration.
    pub fn new_with_evm_factory(chain_spec: Arc<C>, evm_factory: F) -> Self {
        Self {
            chain_spec,
            #[cfg(feature = "std")]
            precompile_cache_map: PrecompileCacheMap::default(),
            #[cfg(feature = "std")]
            precompile_cache_disabled: false,
            evm_factory,
        }
    }

    /// Returns the chain spec associated with this factory.
    pub const fn chain_spec(&self) -> &Arc<C> {
        &self.chain_spec
    }

    /// Returns the configured EVM factory state.
    pub const fn evm_factory(&self) -> &F {
        &self.evm_factory
    }

    /// Returns mutable access to the configured EVM factory state.
    #[cfg(feature = "jit")]
    pub(crate) const fn evm_factory_mut(&mut self) -> &mut F {
        &mut self.evm_factory
    }

    /// Returns a factory with precompile cache disabled, if supported by the active build.
    pub const fn with_precompile_cache_disabled(self, disabled: bool) -> Self {
        #[cfg(feature = "std")]
        {
            let mut this = self;
            this.precompile_cache_disabled = disabled;
            this
        }
        #[cfg(not(feature = "std"))]
        {
            let _ = disabled;
            self
        }
    }

    /// Creates a configured Ethereum block executor.
    pub(crate) fn create_eth_executor<'a>(
        &'a self,
        evm: evm2::Evm<'a, F::Types>,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> EthBlockExecutor<'a, F::Types>
    where
        C: EthChainSpec<Header = Header> + EthereumHardforks,
    {
        EthBlockExecutor::new(
            evm,
            ctx,
            self.chain_spec.as_ref(),
            self.chain_spec.deposit_contract().map(|contract| contract.address),
            HashedStateMode::OutputOnly,
        )
    }

    /// Creates an EVM instance with the configured Ethereum execution environment.
    pub(crate) fn build_evm_with_env<'a, DB>(
        &self,
        db: DB,
        env: EthEvmEnv<F::Types>,
    ) -> evm2::Evm<'a, F::Types>
    where
        C: EthChainSpec<Header = Header>,
        DB: evm2::evm::DynDatabase + 'a,
        F: 'static,
    {
        let spec = env.spec;
        let config = self.evm_factory.execution_config(spec, env.version);
        let registry = self.evm_factory.tx_registry(spec);
        let precompiles = self.evm_factory.precompiles(spec);
        #[cfg(feature = "std")]
        let precompiles: Box<dyn evm2::evm::precompile::PrecompileProvider<F::Types>> =
            if self.precompile_cache_disabled {
                Box::new(precompiles)
            } else {
                Box::new(CachedPrecompileProvider::new(
                    precompiles,
                    self.precompile_cache_map.clone(),
                    spec,
                    None,
                ))
            };
        #[cfg(not(feature = "std"))]
        let precompiles = Box::new(precompiles);

        let mut evm = evm2::Evm::<F::Types>::new_with_execution_config(
            config,
            spec,
            env.block,
            registry,
            db,
            precompiles,
        );
        self.evm_factory.configure_evm(&mut evm);
        evm
    }

    /// Replaces the execution configuration, transaction registry, and precompiles at a segment
    /// boundary while preserving the EVM state and database.
    pub(crate) fn reconfigure_evm<'a>(
        &'a self,
        evm: &mut evm2::Evm<'a, F::Types>,
        env: &EthEvmEnv<F::Types>,
    ) {
        let spec = env.spec;
        let config = self.evm_factory.execution_config(spec, env.version);
        let registry = self.evm_factory.tx_registry(spec);
        let precompiles = self.evm_factory.precompiles(spec);
        #[cfg(feature = "std")]
        let precompiles: Box<dyn evm2::evm::precompile::PrecompileProvider<F::Types>> =
            if self.precompile_cache_disabled {
                Box::new(precompiles)
            } else {
                Box::new(CachedPrecompileProvider::new(
                    precompiles,
                    self.precompile_cache_map.clone(),
                    spec,
                    None,
                ))
            };
        #[cfg(not(feature = "std"))]
        let precompiles = Box::new(precompiles);

        evm.set_block_and_execution_config(env.block, config, spec, registry, precompiles);
        self.evm_factory.configure_evm(evm);
    }
}

impl<C, F> reth_evm::BlockExecutorFactory for EthBlockExecutorFactory<C, F>
where
    C: EthChainSpec<Header = Header> + EthereumHardforks,
    F: EvmFactory,
{
    type EvmFactory = F;
    type EvmTypes = F::Types;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm<'a> = evm2::Evm<'a, F::Types>;
    type EvmEnv = EthEvmEnv<F::Types>;
    type ExecutionCtx<'a>
        = EthBlockExecutionCtx<'a>
    where
        Self: 'a;
    type Executor<'a>
        = EthBlockExecutor<'a, F::Types>
    where
        Self: 'a;

    fn create_executor<'a>(
        &'a self,
        evm: Self::Evm<'a>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a>
    where
        Self: 'a,
    {
        self.create_eth_executor(evm, ctx)
    }

    fn evm_factory(&self) -> &Self::EvmFactory {
        Self::evm_factory(self)
    }

    fn evm_with_env<'a, DB>(&self, db: DB, evm_env: Self::EvmEnv) -> Self::Evm<'a>
    where
        DB: evm2::evm::DynDatabase + 'a,
    {
        self.build_evm_with_env(db, evm_env)
    }
}

/// Reth EVM factory configuration.
///
/// With the `jit` feature, this owns the shared evm2 JIT backend. EVMs only install the JIT
/// interpreter runner when local JIT support was selected through
/// [`ConfigureEvm::with_jit_support`](reth_evm::ConfigureEvm::with_jit_support).
#[derive(Debug, Clone)]
pub struct RethEvmFactory {
    #[cfg(feature = "jit")]
    backend: JitBackend,
    #[cfg(feature = "jit")]
    metrics: JitMetrics,
    #[cfg(feature = "jit")]
    jit_support: bool,
}

impl Default for RethEvmFactory {
    fn default() -> Self {
        Self::disabled()
    }
}

impl RethEvmFactory {
    /// Creates a factory configuration with JIT compilation disabled.
    #[cfg_attr(not(feature = "jit"), allow(clippy::missing_const_for_fn))]
    pub fn disabled() -> Self {
        #[cfg(feature = "jit")]
        {
            Self::new(JitBackend::disabled())
        }
        #[cfg(not(feature = "jit"))]
        {
            Self {}
        }
    }
}

impl EvmFactory for RethEvmFactory {
    type Types = evm2::BaseEvmTypes;
    type SpecId = evm2::SpecId;

    fn spec_id(&self, spec: evm2::SpecId) -> evm2::SpecId {
        spec
    }

    fn tx_registry(
        &self,
        spec: Self::SpecId,
    ) -> evm2::registry::TxRegistry<Self::Types, evm2::TxResult<Self::Types>> {
        evm2::ethereum::ethereum_tx_registry(spec)
    }

    fn configure_evm(&self, evm: &mut evm2::Evm<'_, Self::Types>) {
        #[cfg(feature = "jit")]
        self.configure_evm(evm);

        #[cfg(not(feature = "jit"))]
        let _ = evm;
    }
}

#[cfg(feature = "jit")]
impl RethEvmFactory {
    /// Creates a new factory configuration that owns the backend.
    pub fn new(backend: JitBackend) -> Self {
        Self::new_with_metrics(backend, JitMetrics::default())
    }

    /// Creates a new factory configuration that owns the backend and records metrics.
    pub const fn new_with_metrics(backend: JitBackend, metrics: JitMetrics) -> Self {
        Self { backend, metrics, jit_support: false }
    }

    /// Returns a reference to the JIT backend.
    pub const fn backend(&self) -> &JitBackend {
        &self.backend
    }

    /// Enables or disables local JIT support for subsequently created EVMs.
    pub const fn set_jit_support(&mut self, enabled: bool) {
        self.jit_support = enabled;
    }

    /// Returns whether subsequently created EVMs install the JIT interpreter runner.
    pub const fn jit_support_enabled(&self) -> bool {
        self.jit_support
    }

    /// Installs the evm2 JIT interpreter runner on a configured EVM if locally enabled.
    fn configure_evm(&self, evm: &mut evm2::Evm<'_, evm2::BaseEvmTypes>) {
        if self.jit_support_enabled() {
            evm.set_interpreter_runner(evm2_jit::evm2_evm::JitInterpreterRunner::new(
                self.backend.clone(),
            ));
        }
    }

    /// Pauses JIT helper execution while keeping queueing and resident lookups enabled.
    fn pause_jit(&self) {
        let was_paused = self.backend.is_paused();
        self.backend.pause();
        let is_paused = self.backend.is_paused();
        if !was_paused && is_paused {
            self.metrics.pauses_total.increment(1);
        }
        self.metrics.paused.set(is_paused as u8 as f64);
    }

    /// Resumes background JIT promotion.
    fn resume_jit(&self) {
        let was_paused = self.backend.is_paused();
        self.backend.resume();
        let is_paused = self.backend.is_paused();
        if was_paused && !is_paused {
            self.metrics.resumes_total.increment(1);
        }
        self.metrics.paused.set(is_paused as u8 as f64);
    }
}

#[cfg(feature = "jit")]
impl reth_evm::JitBackend for RethEvmFactory {
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.backend.set_enabled(enabled).map_err(|err| err.to_string())
    }

    fn pause(&self) {
        self.pause_jit();
    }

    fn resume(&self) {
        self.resume_jit();
    }

    fn clear(&self) {
        self.backend.clear_all();
    }
}

/// Prometheus metrics for evm2 JIT runtime stats.
#[cfg(feature = "jit")]
#[derive(reth_metrics::Metrics, Clone)]
#[metrics(scope = "evm2.jit")]
pub struct JitMetrics {
    /// Total lookups that returned a compiled function.
    pub lookup_hits: metrics::Gauge,
    /// Total lookups that returned interpret (not ready).
    pub lookup_misses: metrics::Gauge,
    /// Lookup-observed events currently queued.
    pub events_queued: metrics::Gauge,
    /// Lookup-observed events dropped due to event queue overflow.
    pub events_dropped: metrics::Gauge,
    /// Control commands dropped because the command channel was full.
    pub commands_dropped: metrics::Gauge,
    /// Number of entries in the resident compiled map.
    pub resident_entries: metrics::Gauge,
    /// Approximate total bytes of compiled machine code in the resident map.
    pub jit_code_bytes: metrics::Gauge,
    /// Approximate total bytes of JIT-related data.
    pub jit_data_bytes: metrics::Gauge,
    /// Number of pending control commands queued for the backend.
    pub command_queue_len: metrics::Gauge,
    /// Number of compilation jobs dispatched but not completed yet.
    pub pending_jobs: metrics::Gauge,
    /// Total number of entries evicted.
    pub evictions: metrics::Gauge,
    /// Total number of compilations dispatched.
    pub compilations_dispatched: metrics::Gauge,
    /// Total number of successful compilations.
    pub compilations_succeeded: metrics::Gauge,
    /// Total number of failed compilations.
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
    /// Total number of JIT helper pause requests.
    pub jit_helper_pause_requests: metrics::Gauge,
    /// Total number of JIT helper pause acknowledgements.
    pub jit_helper_pause_acknowledgements: metrics::Gauge,
    /// Total number of JIT helper pause failures.
    pub jit_helper_pause_failures: metrics::Gauge,
    /// Total number of JIT helper pause acknowledgement timeouts.
    pub jit_helper_pause_timeouts: metrics::Gauge,
    /// Total number of JIT helper resume requests.
    pub jit_helper_resume_requests: metrics::Gauge,
    /// Total number of JIT helper resume failures.
    pub jit_helper_resume_failures: metrics::Gauge,
    /// Total number of transitions into paused JIT helper execution.
    pub pauses_total: metrics::Counter,
    /// Total number of transitions out of paused JIT helper execution.
    pub resumes_total: metrics::Counter,
    /// Whether JIT helper execution is currently paused.
    pub paused: metrics::Gauge,
    /// Histogram of total JIT compilation durations in seconds.
    pub jit_compilation_duration: metrics::Histogram,
    /// Duration of the last JIT compilation in seconds.
    pub jit_compilation_duration_last: metrics::Gauge,
    /// Histogram of parse phase durations in seconds.
    pub jit_parse_duration: metrics::Histogram,
    /// Histogram of translate phase durations in seconds.
    pub jit_translate_duration: metrics::Histogram,
    /// Histogram of optimize phase durations in seconds.
    pub jit_optimize_duration: metrics::Histogram,
    /// Histogram of codegen phase durations in seconds.
    pub jit_codegen_duration: metrics::Histogram,
}

#[cfg(feature = "jit")]
impl JitMetrics {
    /// Records a [`RuntimeStatsSnapshot`] into the metrics.
    pub fn record(&self, stats: &RuntimeStatsSnapshot) {
        let RuntimeStatsSnapshot {
            lookup_hits,
            lookup_misses,
            events_dropped,
            commands_dropped,
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
            jit_helper_pause_requests,
            jit_helper_pause_acknowledgements,
            jit_helper_pause_failures,
            jit_helper_pause_timeouts,
            jit_helper_resume_requests,
            jit_helper_resume_failures,
        } = *stats;
        self.lookup_hits.set(lookup_hits as f64);
        self.lookup_misses.set(lookup_misses as f64);
        self.events_queued.set(events_queued as f64);
        self.events_dropped.set(events_dropped as f64);
        self.commands_dropped.set(commands_dropped as f64);
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
        self.jit_helper_pause_requests.set(jit_helper_pause_requests as f64);
        self.jit_helper_pause_acknowledgements.set(jit_helper_pause_acknowledgements as f64);
        self.jit_helper_pause_failures.set(jit_helper_pause_failures as f64);
        self.jit_helper_pause_timeouts.set(jit_helper_pause_timeouts as f64);
        self.jit_helper_resume_requests.set(jit_helper_resume_requests as f64);
        self.jit_helper_resume_failures.set(jit_helper_resume_failures as f64);
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
