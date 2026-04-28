//! clap [Args](clap::Args) for engine purposes

use clap::{builder::Resettable, Args};
use eyre::ensure;
use reth_cli_util::{parse_duration_from_secs_or_ms, parsers::format_duration_as_secs_or_ms};
use reth_engine_primitives::{
    TreeConfig, DEFAULT_INVALID_HEADER_HIT_EVICTION_THRESHOLD, DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE,
    DEFAULT_PERSISTENCE_BACKPRESSURE_THRESHOLD, DEFAULT_SPARSE_TRIE_MAX_HOT_ACCOUNTS,
    DEFAULT_SPARSE_TRIE_MAX_HOT_SLOTS,
};
use std::{sync::OnceLock, time::Duration};

use crate::node_config::{
    DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB, DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
    DEFAULT_PERSISTENCE_THRESHOLD, DEFAULT_RESERVED_CPU_CORES,
};

/// Global static engine defaults
static ENGINE_DEFAULTS: OnceLock<DefaultEngineValues> = OnceLock::new();

/// Default values for engine that can be customized
///
/// Global defaults can be set via [`DefaultEngineValues::try_init`].
#[derive(Debug, Clone)]
pub struct DefaultEngineValues {
    persistence_threshold: u64,
    persistence_backpressure_threshold: u64,
    memory_block_buffer_target: u64,
    invalid_header_hit_eviction_threshold: u8,
    legacy_state_root_task_enabled: bool,
    state_cache_disabled: bool,
    prewarming_disabled: bool,
    state_provider_metrics: bool,
    cross_block_cache_size: usize,
    state_root_task_compare_updates: bool,
    accept_execution_requests_hash: bool,
    multiproof_chunk_size: usize,
    reserved_cpu_cores: usize,
    precompile_cache_disabled: bool,
    state_root_fallback: bool,
    always_process_payload_attributes_on_canonical_head: bool,
    allow_unwind_canonical_header: bool,
    storage_worker_count: Option<usize>,
    account_worker_count: Option<usize>,
    prewarming_threads: Option<usize>,
    cache_metrics_disabled: bool,
    sparse_trie_max_hot_slots: usize,
    sparse_trie_max_hot_accounts: usize,
    slow_block_threshold: Option<Duration>,
    disable_sparse_trie_cache_pruning: bool,
    state_root_task_timeout: Option<String>,
    share_execution_cache_with_payload_builder: bool,
    share_sparse_trie_with_payload_builder: bool,
    suppress_persistence_during_build: bool,
    bal_parallel_execution_disabled: bool,
    bal_parallel_state_root_disabled: bool,
}

impl DefaultEngineValues {
    /// Initialize the global engine defaults with this configuration
    pub fn try_init(self) -> Result<(), Self> {
        ENGINE_DEFAULTS.set(self)
    }

    /// Get a reference to the global engine defaults
    pub fn get_global() -> &'static Self {
        ENGINE_DEFAULTS.get_or_init(Self::default)
    }

    /// Set the default persistence threshold
    pub const fn with_persistence_threshold(mut self, v: u64) -> Self {
        self.persistence_threshold = v;
        self
    }

    /// Set the default persistence backpressure threshold
    pub const fn with_persistence_backpressure_threshold(mut self, v: u64) -> Self {
        self.persistence_backpressure_threshold = v;
        self
    }

    /// Set the default memory block buffer target
    pub const fn with_memory_block_buffer_target(mut self, v: u64) -> Self {
        self.memory_block_buffer_target = v;
        self
    }

    /// Set the invalid header cache hit eviction threshold
    pub const fn with_invalid_header_hit_eviction_threshold(mut self, v: u8) -> Self {
        self.invalid_header_hit_eviction_threshold = v;
        self
    }

    /// Set whether to enable legacy state root task by default
    pub const fn with_legacy_state_root_task_enabled(mut self, v: bool) -> Self {
        self.legacy_state_root_task_enabled = v;
        self
    }

    /// Set whether to disable state cache by default
    pub const fn with_state_cache_disabled(mut self, v: bool) -> Self {
        self.state_cache_disabled = v;
        self
    }

    /// Set whether to disable prewarming by default
    pub const fn with_prewarming_disabled(mut self, v: bool) -> Self {
        self.prewarming_disabled = v;
        self
    }

    /// Set whether to enable state provider metrics by default
    pub const fn with_state_provider_metrics(mut self, v: bool) -> Self {
        self.state_provider_metrics = v;
        self
    }

    /// Set the default cross-block cache size in MB
    pub const fn with_cross_block_cache_size(mut self, v: usize) -> Self {
        self.cross_block_cache_size = v;
        self
    }

    /// Set whether to compare state root task updates by default
    pub const fn with_state_root_task_compare_updates(mut self, v: bool) -> Self {
        self.state_root_task_compare_updates = v;
        self
    }

    /// Set whether to accept execution requests hash by default
    pub const fn with_accept_execution_requests_hash(mut self, v: bool) -> Self {
        self.accept_execution_requests_hash = v;
        self
    }

    /// Set the default multiproof chunk size
    pub const fn with_multiproof_chunk_size(mut self, v: usize) -> Self {
        self.multiproof_chunk_size = v;
        self
    }

    /// Set the default number of reserved CPU cores
    pub const fn with_reserved_cpu_cores(mut self, v: usize) -> Self {
        self.reserved_cpu_cores = v;
        self
    }

    /// Set whether to disable precompile cache by default
    pub const fn with_precompile_cache_disabled(mut self, v: bool) -> Self {
        self.precompile_cache_disabled = v;
        self
    }

    /// Set whether to enable state root fallback by default
    pub const fn with_state_root_fallback(mut self, v: bool) -> Self {
        self.state_root_fallback = v;
        self
    }

    /// Set whether to always process payload attributes on canonical head by default
    pub const fn with_always_process_payload_attributes_on_canonical_head(
        mut self,
        v: bool,
    ) -> Self {
        self.always_process_payload_attributes_on_canonical_head = v;
        self
    }

    /// Set whether to allow unwinding canonical header by default
    pub const fn with_allow_unwind_canonical_header(mut self, v: bool) -> Self {
        self.allow_unwind_canonical_header = v;
        self
    }

    /// Set the default storage worker count
    pub const fn with_storage_worker_count(mut self, v: Option<usize>) -> Self {
        self.storage_worker_count = v;
        self
    }

    /// Set the default account worker count
    pub const fn with_account_worker_count(mut self, v: Option<usize>) -> Self {
        self.account_worker_count = v;
        self
    }

    /// Set the default prewarming thread count
    pub const fn with_prewarming_threads(mut self, v: Option<usize>) -> Self {
        self.prewarming_threads = v;
        self
    }

    /// Set whether to disable cache metrics by default
    pub const fn with_cache_metrics_disabled(mut self, v: bool) -> Self {
        self.cache_metrics_disabled = v;
        self
    }

    /// Set the LFU hot-slot capacity for sparse trie pruning by default
    pub const fn with_sparse_trie_max_hot_slots(mut self, v: usize) -> Self {
        self.sparse_trie_max_hot_slots = v;
        self
    }

    /// Set the LFU hot-account capacity for sparse trie pruning by default
    pub const fn with_sparse_trie_max_hot_accounts(mut self, v: usize) -> Self {
        self.sparse_trie_max_hot_accounts = v;
        self
    }

    /// Set the default slow block threshold.
    pub const fn with_slow_block_threshold(mut self, v: Option<Duration>) -> Self {
        self.slow_block_threshold = v;
        self
    }

    /// Set whether to disable sparse trie cache pruning by default
    pub const fn with_disable_sparse_trie_cache_pruning(mut self, v: bool) -> Self {
        self.disable_sparse_trie_cache_pruning = v;
        self
    }

    /// Set the default state root task timeout
    pub fn with_state_root_task_timeout(mut self, v: Option<String>) -> Self {
        self.state_root_task_timeout = v;
        self
    }

    /// Set whether to share the execution cache with the payload builder by default
    pub const fn with_share_execution_cache_with_payload_builder(mut self, v: bool) -> Self {
        self.share_execution_cache_with_payload_builder = v;
        self
    }

    /// Set whether to share the sparse trie with the payload builder by default
    pub const fn with_share_sparse_trie_with_payload_builder(mut self, v: bool) -> Self {
        self.share_sparse_trie_with_payload_builder = v;
        self
    }

    /// Set whether to suppress persistence during payload building by default
    pub const fn with_suppress_persistence_during_build(mut self, v: bool) -> Self {
        self.suppress_persistence_during_build = v;
        self
    }

    /// Set whether to disable BAL-based parallel execution by default
    pub const fn with_bal_parallel_execution_disabled(mut self, v: bool) -> Self {
        self.bal_parallel_execution_disabled = v;
        self
    }

    /// Set whether to disable BAL-driven parallel state root by default
    pub const fn with_bal_parallel_state_root_disabled(mut self, v: bool) -> Self {
        self.bal_parallel_state_root_disabled = v;
        self
    }
}

impl Default for DefaultEngineValues {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            persistence_backpressure_threshold: DEFAULT_PERSISTENCE_BACKPRESSURE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            invalid_header_hit_eviction_threshold: DEFAULT_INVALID_HEADER_HIT_EVICTION_THRESHOLD,
            legacy_state_root_task_enabled: false,
            state_cache_disabled: false,
            prewarming_disabled: false,
            state_provider_metrics: false,
            cross_block_cache_size: DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB,
            state_root_task_compare_updates: false,
            accept_execution_requests_hash: false,
            multiproof_chunk_size: DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            precompile_cache_disabled: false,
            state_root_fallback: false,
            always_process_payload_attributes_on_canonical_head: false,
            allow_unwind_canonical_header: false,
            storage_worker_count: None,
            account_worker_count: None,
            prewarming_threads: None,
            cache_metrics_disabled: false,
            sparse_trie_max_hot_slots: DEFAULT_SPARSE_TRIE_MAX_HOT_SLOTS,
            sparse_trie_max_hot_accounts: DEFAULT_SPARSE_TRIE_MAX_HOT_ACCOUNTS,
            slow_block_threshold: None,
            disable_sparse_trie_cache_pruning: false,
            state_root_task_timeout: Some("1s".to_string()),
            share_execution_cache_with_payload_builder: false,
            share_sparse_trie_with_payload_builder: false,
            suppress_persistence_during_build: false,
            bal_parallel_execution_disabled: true,
            bal_parallel_state_root_disabled: false,
        }
    }
}

/// Parameters for configuring the engine driver.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Engine")]
pub struct EngineArgs {
    /// Configure persistence threshold for the engine. This determines how many canonical blocks
    /// must be in-memory, ahead of the last persisted block, before flushing canonical blocks to
    /// disk again.
    ///
    /// To persist blocks as fast as the node receives them, set this value to zero. This will
    /// cause more frequent DB writes.
    #[arg(long = "engine.persistence-threshold", default_value_t = DefaultEngineValues::get_global().persistence_threshold)]
    pub persistence_threshold: u64,

    /// Configure the maximum canonical-minus-persisted gap before engine API processing stalls.
    ///
    /// This value must be greater than `--engine.persistence-threshold`.
    #[arg(long = "engine.persistence-backpressure-threshold", default_value_t = DefaultEngineValues::get_global().persistence_backpressure_threshold)]
    pub persistence_backpressure_threshold: u64,

    /// Configure the target number of blocks to keep in memory.
    #[arg(long = "engine.memory-block-buffer-target", default_value_t = DefaultEngineValues::get_global().memory_block_buffer_target)]
    pub memory_block_buffer_target: u64,

    /// Configure how many cache hits an invalid header can accumulate before it is evicted and
    /// reprocessed.
    ///
    /// Set to `0` to effectively disable the cache because entries are evicted on the first
    /// lookup.
    #[arg(long = "engine.invalid-header-cache-hit-eviction-threshold", default_value_t = DefaultEngineValues::get_global().invalid_header_hit_eviction_threshold)]
    pub invalid_header_hit_eviction_threshold: u8,

    /// Enable legacy state root
    #[arg(long = "engine.legacy-state-root", default_value_t = DefaultEngineValues::get_global().legacy_state_root_task_enabled)]
    pub legacy_state_root_task_enabled: bool,

    /// CAUTION: This CLI flag has no effect anymore, use --engine.disable-caching-and-prewarming
    /// if you want to disable caching and prewarming
    #[arg(long = "engine.caching-and-prewarming", default_value = "true", hide = true)]
    #[deprecated]
    pub caching_and_prewarming_enabled: bool,

    /// Disable state cache
    #[arg(long = "engine.disable-state-cache", default_value_t = DefaultEngineValues::get_global().state_cache_disabled)]
    pub state_cache_disabled: bool,

    /// Disable parallel prewarming
    #[arg(long = "engine.disable-prewarming", alias = "engine.disable-caching-and-prewarming", default_value_t = DefaultEngineValues::get_global().prewarming_disabled)]
    pub prewarming_disabled: bool,

    /// CAUTION: This CLI flag has no effect anymore. The parallel sparse trie is always enabled.
    #[deprecated]
    #[arg(long = "engine.parallel-sparse-trie", default_value = "true", hide = true)]
    pub parallel_sparse_trie_enabled: bool,

    /// CAUTION: This CLI flag has no effect anymore. The parallel sparse trie is always enabled.
    #[deprecated]
    #[arg(long = "engine.disable-parallel-sparse-trie", default_value = "false", hide = true)]
    pub parallel_sparse_trie_disabled: bool,

    /// Enable state provider latency metrics. This allows the engine to collect and report stats
    /// about how long state provider calls took during execution, but this does introduce slight
    /// overhead to state provider calls.
    #[arg(long = "engine.state-provider-metrics", default_value_t = DefaultEngineValues::get_global().state_provider_metrics)]
    pub state_provider_metrics: bool,

    /// Configure the size of cross-block cache in megabytes
    #[arg(long = "engine.cross-block-cache-size", default_value_t = DefaultEngineValues::get_global().cross_block_cache_size)]
    pub cross_block_cache_size: usize,

    /// Enable comparing trie updates from the state root task to the trie updates from the regular
    /// state root calculation.
    #[arg(long = "engine.state-root-task-compare-updates", default_value_t = DefaultEngineValues::get_global().state_root_task_compare_updates)]
    pub state_root_task_compare_updates: bool,

    /// Enables accepting requests hash instead of an array of requests in `engine_newPayloadV4`.
    #[arg(long = "engine.accept-execution-requests-hash", default_value_t = DefaultEngineValues::get_global().accept_execution_requests_hash)]
    pub accept_execution_requests_hash: bool,

    /// Multiproof task chunk size for proof targets.
    #[arg(long = "engine.multiproof-chunk-size", default_value_t = DefaultEngineValues::get_global().multiproof_chunk_size)]
    pub multiproof_chunk_size: usize,

    /// Configure the number of reserved CPU cores for non-reth processes
    #[arg(long = "engine.reserved-cpu-cores", default_value_t = DefaultEngineValues::get_global().reserved_cpu_cores)]
    pub reserved_cpu_cores: usize,

    /// CAUTION: This CLI flag has no effect anymore, use --engine.disable-precompile-cache
    /// if you want to disable precompile cache
    #[arg(long = "engine.precompile-cache", default_value = "true", hide = true)]
    #[deprecated]
    pub precompile_cache_enabled: bool,

    /// Disable precompile cache
    #[arg(long = "engine.disable-precompile-cache", default_value_t = DefaultEngineValues::get_global().precompile_cache_disabled)]
    pub precompile_cache_disabled: bool,

    /// Enable state root fallback, useful for testing
    #[arg(long = "engine.state-root-fallback", default_value_t = DefaultEngineValues::get_global().state_root_fallback)]
    pub state_root_fallback: bool,

    /// Always process payload attributes and begin a payload build process even if
    /// `forkchoiceState.headBlockHash` is already the canonical head or an ancestor. See
    /// `TreeConfig::always_process_payload_attributes_on_canonical_head` for more details.
    ///
    /// Note: This is a no-op on OP Stack.
    #[arg(
        long = "engine.always-process-payload-attributes-on-canonical-head",
        default_value_t = DefaultEngineValues::get_global().always_process_payload_attributes_on_canonical_head
    )]
    pub always_process_payload_attributes_on_canonical_head: bool,

    /// Allow unwinding canonical header to ancestor during forkchoice updates.
    /// See `TreeConfig::unwind_canonical_header` for more details.
    #[arg(long = "engine.allow-unwind-canonical-header", default_value_t = DefaultEngineValues::get_global().allow_unwind_canonical_header)]
    pub allow_unwind_canonical_header: bool,

    /// Configure the number of storage proof workers in the Tokio blocking pool.
    /// If not specified, defaults to 2x available parallelism.
    #[arg(long = "engine.storage-worker-count", default_value = Resettable::from(DefaultEngineValues::get_global().storage_worker_count.map(|v| v.to_string().into())))]
    pub storage_worker_count: Option<usize>,

    /// Configure the number of account proof workers in the Tokio blocking pool.
    /// If not specified, defaults to the same count as storage workers.
    #[arg(long = "engine.account-worker-count", default_value = Resettable::from(DefaultEngineValues::get_global().account_worker_count.map(|v| v.to_string().into())))]
    pub account_worker_count: Option<usize>,

    /// Configure the number of prewarming threads.
    /// If not specified, defaults to available parallelism.
    #[arg(long = "engine.prewarming-threads", default_value = Resettable::from(DefaultEngineValues::get_global().prewarming_threads.map(|v| v.to_string().into())))]
    pub prewarming_threads: Option<usize>,

    /// Disable cache metrics recording, which can take up to 50ms with large cached state.
    #[arg(long = "engine.disable-cache-metrics", default_value_t = DefaultEngineValues::get_global().cache_metrics_disabled)]
    pub cache_metrics_disabled: bool,

    /// LFU hot-slot capacity: max storage slots retained across sparse trie prune cycles.
    #[arg(long = "engine.sparse-trie-max-hot-slots", alias = "engine.sparse-trie-max-storage-tries", default_value_t = DefaultEngineValues::get_global().sparse_trie_max_hot_slots)]
    pub sparse_trie_max_hot_slots: usize,

    /// LFU hot-account capacity: max account addresses retained across sparse trie prune cycles.
    #[arg(long = "engine.sparse-trie-max-hot-accounts", default_value_t = DefaultEngineValues::get_global().sparse_trie_max_hot_accounts)]
    pub sparse_trie_max_hot_accounts: usize,

    /// Configure the slow block logging threshold in milliseconds.
    ///
    /// When set, blocks that take longer than this threshold to execute will be logged
    /// with detailed metrics including timing, state operations, and cache statistics.
    ///
    /// Set to 0 to log all blocks (useful for debugging/profiling).
    ///
    /// When not set, slow block logging is disabled (default).
    #[arg(long = "engine.slow-block-threshold", value_parser = parse_duration_from_secs_or_ms, value_name = "DURATION", default_value = Resettable::from(DefaultEngineValues::get_global().slow_block_threshold.map(|threshold| format_duration_as_secs_or_ms(threshold).into())))]
    pub slow_block_threshold: Option<Duration>,

    /// Fully disable sparse trie cache pruning. When set, the cached sparse trie is preserved
    /// without any node pruning or storage trie eviction between blocks. Useful for benchmarking
    /// the effects of retaining the full trie cache.
    #[arg(long = "engine.disable-sparse-trie-cache-pruning", default_value_t = DefaultEngineValues::get_global().disable_sparse_trie_cache_pruning)]
    pub disable_sparse_trie_cache_pruning: bool,

    /// Configure the timeout for the state root task before spawning a sequential fallback.
    /// If the state root task takes longer than this, a sequential computation starts in
    /// parallel and whichever finishes first is used.
    ///
    /// --engine.state-root-task-timeout 1s
    /// --engine.state-root-task-timeout 400ms
    ///
    /// Set to 0s to disable.
    #[arg(
        long = "engine.state-root-task-timeout",
        value_parser = humantime::parse_duration,
        default_value = DefaultEngineValues::get_global().state_root_task_timeout.as_deref().unwrap_or("1s"),
    )]
    pub state_root_task_timeout: Option<Duration>,

    /// Whether to share execution cache with the payload builder.
    ///
    /// When enabled, each payload job will get an instance of cross-block execution cache from the
    /// engine.
    ///
    /// Note: this should only be enabled if node would not be requested to process any payloads in
    /// parallel with payload building.
    #[arg(
        long = "engine.share-execution-cache-with-payload-builder",
        default_value_t = DefaultEngineValues::get_global().share_execution_cache_with_payload_builder,
    )]
    pub share_execution_cache_with_payload_builder: bool,

    /// Whether to share the sparse trie with the payload builder.
    ///
    /// Replaces the payload builder's blocking `state_root_with_updates()` call with the
    /// sparse trie, computing the state root concurrently with transaction execution.
    ///
    /// The engine and payload builder contend for the same trie — if a builder task is
    /// still running when `newPayload` arrives, the engine will block until the trie is
    /// stored back.
    ///
    /// The builder also anchors the trie at the built block's state root, so if the next
    /// `newPayload` is not on top of that block, the trie cache is invalidated and cleared.
    #[arg(
        long = "engine.share-sparse-trie-with-payload-builder",
        default_value_t = DefaultEngineValues::get_global().share_sparse_trie_with_payload_builder,
    )]
    pub share_sparse_trie_with_payload_builder: bool,

    /// Suppress persistence while building a payload.
    ///
    /// When enabled, persistence cycles are deferred from the moment an FCU with payload
    /// attributes arrives until the next FCU clears the build. Useful on chains with short
    /// block times where persistence I/O can interfere with block building latency.
    #[arg(
        long = "engine.suppress-persistence-during-build",
        default_value_t = DefaultEngineValues::get_global().suppress_persistence_during_build,
    )]
    pub suppress_persistence_during_build: bool,

    /// Disable BAL (Block Access List, EIP-7928) based parallel execution. Defaults to disabled,
    /// falling back to transaction-based prewarming even when a BAL is available.
    #[arg(long = "engine.disable-bal-parallel-execution", default_value_t = DefaultEngineValues::get_global().bal_parallel_execution_disabled)]
    pub bal_parallel_execution_disabled: bool,

    /// Disable BAL-driven parallel state root computation. When set, the BAL hashed post state
    /// is not sent to the multiproof task for early parallel state root computation.
    #[arg(long = "engine.disable-bal-parallel-state-root", default_value_t = DefaultEngineValues::get_global().bal_parallel_state_root_disabled)]
    pub bal_parallel_state_root_disabled: bool,

    /// Disable BAL (Block Access List) batched IO during prewarming. When set, falls back
    /// to individual per-slot storage reads instead of batched cursor reads.
    #[arg(long = "engine.disable-bal-batch-io", default_value_t = false)]
    pub disable_bal_batch_io: bool,

    /// Add random jitter before each proof computation (trie-debug only).
    /// Each proof worker sleeps for a random duration up to this value before
    /// starting work. Useful for stress-testing timing-sensitive proof logic.
    ///
    /// --engine.proof-jitter 100ms
    /// --engine.proof-jitter 1s
    #[cfg(feature = "trie-debug")]
    #[arg(
        long = "engine.proof-jitter",
        value_parser = humantime::parse_duration,
    )]
    pub proof_jitter: Option<Duration>,
}

#[allow(deprecated)]
impl Default for EngineArgs {
    fn default() -> Self {
        let DefaultEngineValues {
            persistence_threshold,
            persistence_backpressure_threshold,
            memory_block_buffer_target,
            invalid_header_hit_eviction_threshold,
            legacy_state_root_task_enabled,
            state_cache_disabled,
            prewarming_disabled,
            state_provider_metrics,
            cross_block_cache_size,
            state_root_task_compare_updates,
            accept_execution_requests_hash,
            multiproof_chunk_size,
            reserved_cpu_cores,
            precompile_cache_disabled,
            state_root_fallback,
            always_process_payload_attributes_on_canonical_head,
            allow_unwind_canonical_header,
            storage_worker_count,
            account_worker_count,
            prewarming_threads,
            cache_metrics_disabled,
            sparse_trie_max_hot_slots,
            sparse_trie_max_hot_accounts,
            slow_block_threshold,
            disable_sparse_trie_cache_pruning,
            state_root_task_timeout,
            share_execution_cache_with_payload_builder,
            share_sparse_trie_with_payload_builder,
            suppress_persistence_during_build,
            bal_parallel_execution_disabled,
            bal_parallel_state_root_disabled,
        } = DefaultEngineValues::get_global().clone();
        Self {
            persistence_threshold,
            persistence_backpressure_threshold,
            memory_block_buffer_target,
            invalid_header_hit_eviction_threshold,
            legacy_state_root_task_enabled,
            state_root_task_compare_updates,
            caching_and_prewarming_enabled: true,
            state_cache_disabled,
            prewarming_disabled,
            parallel_sparse_trie_enabled: true,
            parallel_sparse_trie_disabled: false,
            state_provider_metrics,
            cross_block_cache_size,
            accept_execution_requests_hash,
            multiproof_chunk_size,
            reserved_cpu_cores,
            precompile_cache_enabled: true,
            precompile_cache_disabled,
            state_root_fallback,
            always_process_payload_attributes_on_canonical_head,
            allow_unwind_canonical_header,
            storage_worker_count,
            account_worker_count,
            prewarming_threads,
            cache_metrics_disabled,
            sparse_trie_max_hot_slots,
            sparse_trie_max_hot_accounts,
            slow_block_threshold,
            disable_sparse_trie_cache_pruning,
            state_root_task_timeout: state_root_task_timeout
                .as_deref()
                .map(|s| humantime::parse_duration(s).expect("valid default duration")),
            share_execution_cache_with_payload_builder,
            share_sparse_trie_with_payload_builder,
            suppress_persistence_during_build,
            bal_parallel_execution_disabled,
            bal_parallel_state_root_disabled,
            disable_bal_batch_io: false,
            #[cfg(feature = "trie-debug")]
            proof_jitter: None,
        }
    }
}

impl EngineArgs {
    /// Validates cross-field engine arguments.
    pub fn validate(&self) -> eyre::Result<()> {
        ensure!(
            self.persistence_backpressure_threshold > self.persistence_threshold,
            "--engine.persistence-backpressure-threshold ({}) must be greater than --engine.persistence-threshold ({})",
            self.persistence_backpressure_threshold,
            self.persistence_threshold
        );
        Ok(())
    }

    /// Creates a [`TreeConfig`] from the engine arguments.
    pub fn tree_config(&self) -> TreeConfig {
        let config = TreeConfig::default()
            .with_persistence_threshold(self.persistence_threshold)
            .with_persistence_backpressure_threshold(self.persistence_backpressure_threshold)
            .with_memory_block_buffer_target(self.memory_block_buffer_target)
            .with_invalid_header_hit_eviction_threshold(self.invalid_header_hit_eviction_threshold)
            .with_legacy_state_root(self.legacy_state_root_task_enabled)
            .without_state_cache(self.state_cache_disabled)
            .without_prewarming(self.prewarming_disabled)
            .with_state_provider_metrics(self.state_provider_metrics)
            .with_always_compare_trie_updates(self.state_root_task_compare_updates)
            .with_cross_block_cache_size(self.cross_block_cache_size * 1024 * 1024)
            .with_multiproof_chunk_size(self.multiproof_chunk_size)
            .with_reserved_cpu_cores(self.reserved_cpu_cores)
            .without_precompile_cache(self.precompile_cache_disabled)
            .with_state_root_fallback(self.state_root_fallback)
            .with_always_process_payload_attributes_on_canonical_head(
                self.always_process_payload_attributes_on_canonical_head,
            )
            .with_unwind_canonical_header(self.allow_unwind_canonical_header)
            .without_cache_metrics(self.cache_metrics_disabled)
            .with_sparse_trie_max_hot_slots(self.sparse_trie_max_hot_slots)
            .with_sparse_trie_max_hot_accounts(self.sparse_trie_max_hot_accounts)
            .with_slow_block_threshold(self.slow_block_threshold)
            .with_disable_sparse_trie_cache_pruning(self.disable_sparse_trie_cache_pruning)
            .with_state_root_task_timeout(self.state_root_task_timeout.filter(|d| !d.is_zero()))
            .with_share_execution_cache_with_payload_builder(
                self.share_execution_cache_with_payload_builder,
            )
            .with_share_sparse_trie_with_payload_builder(
                self.share_sparse_trie_with_payload_builder,
            )
            .with_suppress_persistence_during_build(self.suppress_persistence_during_build)
            .without_bal_parallel_execution(self.bal_parallel_execution_disabled)
            .without_bal_parallel_state_root(self.bal_parallel_state_root_disabled)
            .without_bal_batch_io(self.disable_bal_batch_io);
        #[cfg(feature = "trie-debug")]
        let config = config.with_proof_jitter(self.proof_jitter);
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_parse_engine_args() {
        let default_args = EngineArgs::default();
        let args = CommandParser::<EngineArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    #[allow(deprecated)]
    fn engine_args() {
        let args = EngineArgs {
            persistence_threshold: 100,
            persistence_backpressure_threshold: 101,
            memory_block_buffer_target: 50,
            invalid_header_hit_eviction_threshold: 7,
            legacy_state_root_task_enabled: true,
            caching_and_prewarming_enabled: true,
            state_cache_disabled: true,
            prewarming_disabled: true,
            parallel_sparse_trie_enabled: true,
            parallel_sparse_trie_disabled: false,
            state_provider_metrics: true,
            cross_block_cache_size: 256,
            state_root_task_compare_updates: true,
            accept_execution_requests_hash: true,
            multiproof_chunk_size: 512,
            reserved_cpu_cores: 4,
            precompile_cache_enabled: true,
            precompile_cache_disabled: true,
            state_root_fallback: true,
            always_process_payload_attributes_on_canonical_head: true,
            allow_unwind_canonical_header: true,
            storage_worker_count: Some(16),
            account_worker_count: Some(8),
            prewarming_threads: Some(4),
            cache_metrics_disabled: true,
            sparse_trie_max_hot_slots: 100,
            sparse_trie_max_hot_accounts: 500,
            slow_block_threshold: None,
            disable_sparse_trie_cache_pruning: true,
            state_root_task_timeout: Some(Duration::from_secs(2)),
            share_execution_cache_with_payload_builder: false,
            share_sparse_trie_with_payload_builder: false,
            suppress_persistence_during_build: false,
            bal_parallel_execution_disabled: true,
            bal_parallel_state_root_disabled: true,
            disable_bal_batch_io: true,
            #[cfg(feature = "trie-debug")]
            proof_jitter: None,
        };

        let parsed_args = CommandParser::<EngineArgs>::parse_from([
            "reth",
            "--engine.persistence-threshold",
            "100",
            "--engine.persistence-backpressure-threshold",
            "101",
            "--engine.memory-block-buffer-target",
            "50",
            "--engine.invalid-header-cache-hit-eviction-threshold",
            "7",
            "--engine.legacy-state-root",
            "--engine.disable-state-cache",
            "--engine.disable-prewarming",
            "--engine.state-provider-metrics",
            "--engine.cross-block-cache-size",
            "256",
            "--engine.state-root-task-compare-updates",
            "--engine.accept-execution-requests-hash",
            "--engine.multiproof-chunk-size",
            "512",
            "--engine.reserved-cpu-cores",
            "4",
            "--engine.disable-precompile-cache",
            "--engine.state-root-fallback",
            "--engine.always-process-payload-attributes-on-canonical-head",
            "--engine.allow-unwind-canonical-header",
            "--engine.storage-worker-count",
            "16",
            "--engine.account-worker-count",
            "8",
            "--engine.prewarming-threads",
            "4",
            "--engine.disable-cache-metrics",
            "--engine.sparse-trie-max-hot-slots",
            "100",
            "--engine.sparse-trie-max-hot-accounts",
            "500",
            "--engine.disable-sparse-trie-cache-pruning",
            "--engine.state-root-task-timeout",
            "2s",
            "--engine.disable-bal-parallel-execution",
            "--engine.disable-bal-parallel-state-root",
            "--engine.disable-bal-batch-io",
        ])
        .args;

        assert_eq!(parsed_args, args);
    }

    #[test]
    fn validate_rejects_invalid_backpressure_threshold() {
        let args = EngineArgs {
            persistence_threshold: 4,
            persistence_backpressure_threshold: 4,
            ..EngineArgs::default()
        };

        let err = args.validate().unwrap_err().to_string();
        assert!(err.contains("engine.persistence-backpressure-threshold"));
        assert!(err.contains("engine.persistence-threshold"));
    }

    #[test]
    fn test_parse_slow_block_threshold() {
        // Test default value (None - disabled)
        let args = CommandParser::<EngineArgs>::parse_from(["reth"]).args;
        assert_eq!(args.slow_block_threshold, None);

        // Test setting to 0 (log all blocks)
        let args =
            CommandParser::<EngineArgs>::parse_from(["reth", "--engine.slow-block-threshold", "0"])
                .args;
        assert_eq!(args.slow_block_threshold, Some(Duration::ZERO));

        // Test setting to custom value
        let args = CommandParser::<EngineArgs>::parse_from([
            "reth",
            "--engine.slow-block-threshold",
            "500",
        ])
        .args;
        assert_eq!(args.slow_block_threshold, Some(Duration::from_secs(500)));

        let args = CommandParser::<EngineArgs>::parse_from([
            "reth",
            "--engine.slow-block-threshold",
            "500ms",
        ])
        .args;
        assert_eq!(args.slow_block_threshold, Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_parse_invalid_header_hit_eviction_threshold() {
        let args = CommandParser::<EngineArgs>::parse_from(["reth"]).args;
        assert_eq!(
            args.invalid_header_hit_eviction_threshold,
            DEFAULT_INVALID_HEADER_HIT_EVICTION_THRESHOLD
        );
        assert_eq!(
            args.tree_config().invalid_header_hit_eviction_threshold(),
            DEFAULT_INVALID_HEADER_HIT_EVICTION_THRESHOLD
        );

        let args = CommandParser::<EngineArgs>::parse_from([
            "reth",
            "--engine.invalid-header-cache-hit-eviction-threshold",
            "0",
        ])
        .args;
        assert_eq!(args.invalid_header_hit_eviction_threshold, 0);
        assert_eq!(args.tree_config().invalid_header_hit_eviction_threshold(), 0);
    }

    #[test]
    fn test_parse_share_sparse_trie_flag() {
        let args = CommandParser::<EngineArgs>::parse_from(["reth"]).args;
        assert!(!args.share_sparse_trie_with_payload_builder);
        assert!(!args.tree_config().share_sparse_trie_with_payload_builder());

        let args = CommandParser::<EngineArgs>::parse_from([
            "reth",
            "--engine.share-sparse-trie-with-payload-builder",
        ])
        .args;
        assert!(args.share_sparse_trie_with_payload_builder);
        assert!(args.tree_config().share_sparse_trie_with_payload_builder());
    }
}
