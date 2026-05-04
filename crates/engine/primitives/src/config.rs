//! Engine tree configuration.

use alloy_eips::merge::EPOCH_SLOTS;
use core::time::Duration;

/// Triggers persistence when the number of canonical blocks in memory exceeds this threshold.
pub const DEFAULT_PERSISTENCE_THRESHOLD: u64 = 2;

/// Maximum canonical-minus-persisted gap before engine API processing is stalled.
pub const DEFAULT_PERSISTENCE_BACKPRESSURE_THRESHOLD: u64 = 16;

/// How close to the canonical head we persist blocks.
pub const DEFAULT_MEMORY_BLOCK_BUFFER_TARGET: u64 = 0;

/// The size of proof targets chunk to spawn in one multiproof calculation.
pub const DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE: usize = 5;

/// Default number of cache hits before an invalid header entry is evicted and reprocessed.
pub const DEFAULT_INVALID_HEADER_HIT_EVICTION_THRESHOLD: u8 = 128;

/// Gas threshold below which the small block chunk size is used.
pub const SMALL_BLOCK_GAS_THRESHOLD: u64 = 20_000_000;

/// Default number of reserved CPU cores for non-reth processes.
///
/// This will be deducted from the thread count of main reth global threadpool.
pub const DEFAULT_RESERVED_CPU_CORES: usize = 1;

/// Default depth for sparse trie pruning.
///
/// Nodes at this depth and below are converted to hash stubs to reduce memory.
/// Depth 4 means we keep roughly 16^4 = 65536 potential branch paths at most.
pub const DEFAULT_SPARSE_TRIE_PRUNE_DEPTH: usize = 4;

/// Default LFU hot-slot capacity for sparse trie pruning.
///
/// Limits the number of `(address, slot)` pairs retained across prune cycles.
pub const DEFAULT_SPARSE_TRIE_MAX_HOT_SLOTS: usize = 1500;

/// Default LFU hot-account capacity for sparse trie pruning.
///
/// Limits the number of account addresses retained across prune cycles.
pub const DEFAULT_SPARSE_TRIE_MAX_HOT_ACCOUNTS: usize = 1000;

/// Default timeout for the state root task before spawning a sequential fallback.
pub const DEFAULT_STATE_ROOT_TASK_TIMEOUT: Duration = Duration::from_secs(1);

const DEFAULT_BLOCK_BUFFER_LIMIT: u32 = EPOCH_SLOTS as u32 * 2;
const DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH: u32 = 256;
const DEFAULT_MAX_EXECUTE_BLOCK_BATCH_SIZE: usize = 4;
const DEFAULT_CROSS_BLOCK_CACHE_SIZE: usize = default_cross_block_cache_size();

const fn assert_backpressure_threshold_invariant(
    persistence_threshold: u64,
    persistence_backpressure_threshold: u64,
) {
    debug_assert!(
        persistence_backpressure_threshold > persistence_threshold,
        "persistence_backpressure_threshold must be greater than persistence_threshold",
    );
}

const fn default_cross_block_cache_size() -> usize {
    if cfg!(test) {
        1024 * 1024 // 1 MB in tests
    } else if cfg!(target_pointer_width = "32") {
        usize::MAX // max possible on wasm32 / 32-bit
    } else {
        4 * 1024 * 1024 * 1024 // 4 GB on 64-bit
    }
}

/// Determines if the host has enough parallelism to run the payload processor.
///
/// It requires at least 5 parallel threads:
/// - Engine in main thread that spawns the state root task.
/// - Multiproof task in payload processor
/// - Sparse Trie task in payload processor
/// - Multiproof computation spawned in payload processor
/// - Storage root computation spawned in trie parallel proof
pub fn has_enough_parallelism() -> bool {
    #[cfg(feature = "std")]
    {
        std::thread::available_parallelism().is_ok_and(|num| num.get() >= 5)
    }
    #[cfg(not(feature = "std"))]
    false
}

/// The configuration of the engine tree.
#[derive(Debug, Clone)]
pub struct TreeConfig {
    /// Maximum number of blocks to be kept only in memory without triggering
    /// persistence.
    persistence_threshold: u64,
    /// How close to the canonical head we persist blocks. Represents the ideal
    /// number of most recent blocks to keep in memory for quick access and reorgs.
    ///
    /// Note: this should be less than or equal to `persistence_threshold`.
    memory_block_buffer_target: u64,
    /// Maximum canonical-minus-persisted gap before engine API processing is stalled.
    persistence_backpressure_threshold: u64,
    /// Number of pending blocks that cannot be executed due to missing parent and
    /// are kept in cache.
    block_buffer_limit: u32,
    /// Number of invalid headers to keep in cache.
    max_invalid_header_cache_length: u32,
    /// Number of cache hits before an invalid header entry is evicted and reprocessed.
    ///
    /// Setting this to `0` effectively disables the cache because entries are evicted on the
    /// first lookup.
    invalid_header_hit_eviction_threshold: u8,
    /// Maximum number of blocks to execute sequentially in a batch.
    ///
    /// This is used as a cutoff to prevent long-running sequential block execution when we receive
    /// a batch of downloaded blocks.
    max_execute_block_batch_size: usize,
    /// Whether to use the legacy state root calculation method instead of the
    /// new state root task.
    legacy_state_root: bool,
    /// Whether to always compare trie updates from the state root task to the trie updates from
    /// the regular state root calculation.
    always_compare_trie_updates: bool,
    /// Whether to disable state cache.
    disable_state_cache: bool,
    /// Whether to disable parallel prewarming.
    disable_prewarming: bool,
    /// Whether to enable state provider metrics.
    state_provider_metrics: bool,
    /// Cross-block cache size in bytes.
    cross_block_cache_size: usize,
    /// Whether the host has enough parallelism to run state root task.
    has_enough_parallelism: bool,
    /// Multiproof task chunk size for proof targets.
    multiproof_chunk_size: usize,
    /// Number of reserved CPU cores for non-reth processes
    reserved_cpu_cores: usize,
    /// Whether to disable the precompile cache
    precompile_cache_disabled: bool,
    /// Whether to use state root fallback for testing
    state_root_fallback: bool,
    /// Whether to always process payload attributes and begin a payload build process
    /// even if `forkchoiceState.headBlockHash` is already the canonical head or an ancestor.
    ///
    /// The Engine API specification generally states that client software "MUST NOT begin a
    /// payload build process if `forkchoiceState.headBlockHash` references a `VALID`
    /// ancestor of the head of canonical chain".
    /// See: <https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#engine_forkchoiceupdatedv1> (Rule 2)
    ///
    /// This flag allows overriding that behavior.
    /// This is useful for specific chain configurations (e.g., OP Stack where proposers
    /// can reorg their own chain), various custom chains, or for development/testing purposes
    /// where immediate payload regeneration is desired despite the head not changing or moving to
    /// an ancestor.
    always_process_payload_attributes_on_canonical_head: bool,
    /// Whether to unwind canonical header to ancestor during forkchoice updates.
    allow_unwind_canonical_header: bool,
    /// Whether to disable cache metrics recording (can be expensive with large cached state).
    disable_cache_metrics: bool,
    /// Depth for sparse trie pruning after state root computation.
    sparse_trie_prune_depth: usize,
    /// LFU hot-slot capacity: max `(address, slot)` pairs retained across prune cycles.
    sparse_trie_max_hot_slots: usize,
    /// LFU hot-account capacity: max account addresses retained across prune cycles.
    sparse_trie_max_hot_accounts: usize,
    /// When set, blocks whose total processing time (execution + state reads + state root +
    /// DB commit) exceeds this duration trigger a structured `warn!` log with detailed timing,
    /// state-operation counts, and cache hit-rate metrics. `Duration::ZERO` logs every block.
    slow_block_threshold: Option<Duration>,
    /// Whether to fully disable sparse trie cache pruning between blocks.
    disable_sparse_trie_cache_pruning: bool,
    /// Timeout for the state root task before spawning a sequential fallback computation.
    /// If `Some`, after waiting this duration for the state root task, a sequential state root
    /// computation is spawned in parallel and whichever finishes first is used.
    /// If `None`, the timeout fallback is disabled.
    state_root_task_timeout: Option<Duration>,
    /// Whether to share execution cache with the payload builder.
    share_execution_cache_with_payload_builder: bool,
    /// Whether to share sparse trie with the payload builder.
    share_sparse_trie_with_payload_builder: bool,
    /// Whether to suppress persistence cycles while building a payload.
    ///
    /// When enabled, persistence is deferred from the moment an FCU with payload attributes
    /// arrives until the next FCU without attributes. This avoids persistence I/O competing
    /// with block building on latency-sensitive chains.
    suppress_persistence_during_build: bool,
    /// Whether to disable BAL (Block Access List, EIP-7928) based parallel execution.
    /// When disabled, falls back to transaction-based prewarming even when a BAL is available.
    disable_bal_parallel_execution: bool,
    /// Whether to disable BAL-driven parallel state root computation.
    /// When disabled, the BAL hashed post state is not sent to the multiproof task for
    /// early parallel state root computation.
    disable_bal_parallel_state_root: bool,
    /// Whether to disable BAL (Block Access List) storage prefetch IO during prewarming.
    /// When set, BAL storage slots are not read into the execution cache. BAL hashed-state
    /// streaming for parallel state-root computation is controlled separately.
    disable_bal_batch_io: bool,
    /// Maximum random jitter applied before each proof computation (trie-debug only).
    /// When set, each proof worker sleeps for a random duration up to this value
    /// before starting a proof calculation.
    #[cfg(feature = "trie-debug")]
    proof_jitter: Option<Duration>,
}

impl Default for TreeConfig {
    fn default() -> Self {
        assert_backpressure_threshold_invariant(
            DEFAULT_PERSISTENCE_THRESHOLD,
            DEFAULT_PERSISTENCE_BACKPRESSURE_THRESHOLD,
        );
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            persistence_backpressure_threshold: DEFAULT_PERSISTENCE_BACKPRESSURE_THRESHOLD,
            block_buffer_limit: DEFAULT_BLOCK_BUFFER_LIMIT,
            max_invalid_header_cache_length: DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH,
            invalid_header_hit_eviction_threshold: DEFAULT_INVALID_HEADER_HIT_EVICTION_THRESHOLD,
            max_execute_block_batch_size: DEFAULT_MAX_EXECUTE_BLOCK_BATCH_SIZE,
            legacy_state_root: false,
            always_compare_trie_updates: false,
            disable_state_cache: false,
            disable_prewarming: false,
            state_provider_metrics: false,
            cross_block_cache_size: DEFAULT_CROSS_BLOCK_CACHE_SIZE,
            has_enough_parallelism: has_enough_parallelism(),
            multiproof_chunk_size: DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            precompile_cache_disabled: false,
            state_root_fallback: false,
            always_process_payload_attributes_on_canonical_head: false,
            allow_unwind_canonical_header: false,
            disable_cache_metrics: false,
            sparse_trie_prune_depth: DEFAULT_SPARSE_TRIE_PRUNE_DEPTH,
            sparse_trie_max_hot_slots: DEFAULT_SPARSE_TRIE_MAX_HOT_SLOTS,
            sparse_trie_max_hot_accounts: DEFAULT_SPARSE_TRIE_MAX_HOT_ACCOUNTS,
            slow_block_threshold: None,
            disable_sparse_trie_cache_pruning: false,
            state_root_task_timeout: Some(DEFAULT_STATE_ROOT_TASK_TIMEOUT),
            share_execution_cache_with_payload_builder: false,
            share_sparse_trie_with_payload_builder: false,
            suppress_persistence_during_build: false,
            disable_bal_parallel_execution: true,
            disable_bal_parallel_state_root: false,
            disable_bal_batch_io: false,
            #[cfg(feature = "trie-debug")]
            proof_jitter: None,
        }
    }
}

impl TreeConfig {
    /// Create engine tree configuration.
    #[expect(clippy::too_many_arguments)]
    pub const fn new(
        persistence_threshold: u64,
        memory_block_buffer_target: u64,
        persistence_backpressure_threshold: u64,
        block_buffer_limit: u32,
        max_invalid_header_cache_length: u32,
        invalid_header_hit_eviction_threshold: u8,
        max_execute_block_batch_size: usize,
        legacy_state_root: bool,
        always_compare_trie_updates: bool,
        disable_state_cache: bool,
        disable_prewarming: bool,
        state_provider_metrics: bool,
        cross_block_cache_size: usize,
        has_enough_parallelism: bool,
        multiproof_chunk_size: usize,
        reserved_cpu_cores: usize,
        precompile_cache_disabled: bool,
        state_root_fallback: bool,
        always_process_payload_attributes_on_canonical_head: bool,
        allow_unwind_canonical_header: bool,
        disable_cache_metrics: bool,
        sparse_trie_prune_depth: usize,
        sparse_trie_max_hot_slots: usize,
        sparse_trie_max_hot_accounts: usize,
        slow_block_threshold: Option<Duration>,
        state_root_task_timeout: Option<Duration>,
        share_execution_cache_with_payload_builder: bool,
        share_sparse_trie_with_payload_builder: bool,
    ) -> Self {
        assert_backpressure_threshold_invariant(
            persistence_threshold,
            persistence_backpressure_threshold,
        );
        Self {
            persistence_threshold,
            memory_block_buffer_target,
            persistence_backpressure_threshold,
            block_buffer_limit,
            max_invalid_header_cache_length,
            invalid_header_hit_eviction_threshold,
            max_execute_block_batch_size,
            legacy_state_root,
            always_compare_trie_updates,
            disable_state_cache,
            disable_prewarming,
            state_provider_metrics,
            cross_block_cache_size,
            has_enough_parallelism,
            multiproof_chunk_size,
            reserved_cpu_cores,
            precompile_cache_disabled,
            state_root_fallback,
            always_process_payload_attributes_on_canonical_head,
            allow_unwind_canonical_header,
            disable_cache_metrics,
            sparse_trie_prune_depth,
            sparse_trie_max_hot_slots,
            sparse_trie_max_hot_accounts,
            slow_block_threshold,
            disable_sparse_trie_cache_pruning: false,
            state_root_task_timeout,
            share_execution_cache_with_payload_builder,
            share_sparse_trie_with_payload_builder,
            suppress_persistence_during_build: false,
            disable_bal_parallel_execution: true,
            disable_bal_parallel_state_root: false,
            disable_bal_batch_io: false,
            #[cfg(feature = "trie-debug")]
            proof_jitter: None,
        }
    }

    /// Return the persistence threshold.
    pub const fn persistence_threshold(&self) -> u64 {
        self.persistence_threshold
    }

    /// Return the memory block buffer target.
    pub const fn memory_block_buffer_target(&self) -> u64 {
        self.memory_block_buffer_target
    }

    /// Return the persistence backpressure threshold.
    pub const fn persistence_backpressure_threshold(&self) -> u64 {
        self.persistence_backpressure_threshold
    }

    /// Return the block buffer limit.
    pub const fn block_buffer_limit(&self) -> u32 {
        self.block_buffer_limit
    }

    /// Return the maximum invalid cache header length.
    pub const fn max_invalid_header_cache_length(&self) -> u32 {
        self.max_invalid_header_cache_length
    }

    /// Return the invalid header cache hit eviction threshold.
    ///
    /// Setting this to `0` effectively disables the cache because entries are evicted on the
    /// first lookup.
    pub const fn invalid_header_hit_eviction_threshold(&self) -> u8 {
        self.invalid_header_hit_eviction_threshold
    }

    /// Return the maximum execute block batch size.
    pub const fn max_execute_block_batch_size(&self) -> usize {
        self.max_execute_block_batch_size
    }

    /// Return the multiproof task chunk size.
    pub const fn multiproof_chunk_size(&self) -> usize {
        self.multiproof_chunk_size
    }

    /// Return the effective multiproof task chunk size.
    pub const fn effective_multiproof_chunk_size(&self) -> usize {
        self.multiproof_chunk_size
    }

    /// Return the number of reserved CPU cores for non-reth processes
    pub const fn reserved_cpu_cores(&self) -> usize {
        self.reserved_cpu_cores
    }

    /// Returns whether to use the legacy state root calculation method instead
    /// of the new state root task
    pub const fn legacy_state_root(&self) -> bool {
        self.legacy_state_root
    }

    /// Returns whether or not state provider metrics are enabled.
    pub const fn state_provider_metrics(&self) -> bool {
        self.state_provider_metrics
    }

    /// Returns whether or not state cache is disabled.
    pub const fn disable_state_cache(&self) -> bool {
        self.disable_state_cache
    }

    /// Returns whether or not parallel prewarming is disabled.
    pub const fn disable_prewarming(&self) -> bool {
        self.disable_prewarming
    }

    /// Returns whether to always compare trie updates from the state root task to the trie updates
    /// from the regular state root calculation.
    pub const fn always_compare_trie_updates(&self) -> bool {
        self.always_compare_trie_updates
    }

    /// Returns the cross-block cache size.
    pub const fn cross_block_cache_size(&self) -> usize {
        self.cross_block_cache_size
    }

    /// Returns whether precompile cache is disabled.
    pub const fn precompile_cache_disabled(&self) -> bool {
        self.precompile_cache_disabled
    }

    /// Returns whether to use state root fallback.
    pub const fn state_root_fallback(&self) -> bool {
        self.state_root_fallback
    }

    /// Sets whether to always process payload attributes when the FCU head is already canonical.
    pub const fn with_always_process_payload_attributes_on_canonical_head(
        mut self,
        always_process_payload_attributes_on_canonical_head: bool,
    ) -> Self {
        self.always_process_payload_attributes_on_canonical_head =
            always_process_payload_attributes_on_canonical_head;
        self
    }

    /// Returns true if payload attributes should always be processed even when the FCU head is
    /// canonical.
    pub const fn always_process_payload_attributes_on_canonical_head(&self) -> bool {
        self.always_process_payload_attributes_on_canonical_head
    }

    /// Returns true if canonical header should be unwound to ancestor during forkchoice updates.
    pub const fn unwind_canonical_header(&self) -> bool {
        self.allow_unwind_canonical_header
    }

    /// Setter for persistence threshold.
    pub const fn with_persistence_threshold(mut self, persistence_threshold: u64) -> Self {
        self.persistence_threshold = persistence_threshold;
        assert_backpressure_threshold_invariant(
            self.persistence_threshold,
            self.persistence_backpressure_threshold,
        );
        self
    }

    /// Setter for memory block buffer target.
    pub const fn with_memory_block_buffer_target(
        mut self,
        memory_block_buffer_target: u64,
    ) -> Self {
        self.memory_block_buffer_target = memory_block_buffer_target;
        self
    }

    /// Setter for persistence backpressure threshold.
    pub const fn with_persistence_backpressure_threshold(
        mut self,
        persistence_backpressure_threshold: u64,
    ) -> Self {
        self.persistence_backpressure_threshold = persistence_backpressure_threshold;
        assert_backpressure_threshold_invariant(
            self.persistence_threshold,
            self.persistence_backpressure_threshold,
        );
        self
    }

    /// Setter for block buffer limit.
    pub const fn with_block_buffer_limit(mut self, block_buffer_limit: u32) -> Self {
        self.block_buffer_limit = block_buffer_limit;
        self
    }

    /// Setter for maximum invalid header cache length.
    pub const fn with_max_invalid_header_cache_length(
        mut self,
        max_invalid_header_cache_length: u32,
    ) -> Self {
        self.max_invalid_header_cache_length = max_invalid_header_cache_length;
        self
    }

    /// Setter for the invalid header cache hit eviction threshold.
    pub const fn with_invalid_header_hit_eviction_threshold(
        mut self,
        invalid_header_hit_eviction_threshold: u8,
    ) -> Self {
        self.invalid_header_hit_eviction_threshold = invalid_header_hit_eviction_threshold;
        self
    }

    /// Setter for maximum execute block batch size.
    pub const fn with_max_execute_block_batch_size(
        mut self,
        max_execute_block_batch_size: usize,
    ) -> Self {
        self.max_execute_block_batch_size = max_execute_block_batch_size;
        self
    }

    /// Setter for whether to use the legacy state root calculation method.
    pub const fn with_legacy_state_root(mut self, legacy_state_root: bool) -> Self {
        self.legacy_state_root = legacy_state_root;
        self
    }

    /// Setter for whether to disable state cache.
    pub const fn without_state_cache(mut self, disable_state_cache: bool) -> Self {
        self.disable_state_cache = disable_state_cache;
        self
    }

    /// Setter for whether to disable parallel prewarming.
    pub const fn without_prewarming(mut self, disable_prewarming: bool) -> Self {
        self.disable_prewarming = disable_prewarming;
        self
    }

    /// Setter for whether to always compare trie updates from the state root task to the trie
    /// updates from the regular state root calculation.
    pub const fn with_always_compare_trie_updates(
        mut self,
        always_compare_trie_updates: bool,
    ) -> Self {
        self.always_compare_trie_updates = always_compare_trie_updates;
        self
    }

    /// Setter for cross block cache size.
    pub const fn with_cross_block_cache_size(mut self, cross_block_cache_size: usize) -> Self {
        self.cross_block_cache_size = cross_block_cache_size;
        self
    }

    /// Setter for has enough parallelism.
    pub const fn with_has_enough_parallelism(mut self, has_enough_parallelism: bool) -> Self {
        self.has_enough_parallelism = has_enough_parallelism;
        self
    }

    /// Setter for state provider metrics.
    pub const fn with_state_provider_metrics(mut self, state_provider_metrics: bool) -> Self {
        self.state_provider_metrics = state_provider_metrics;
        self
    }

    /// Setter for multiproof task chunk size for proof targets.
    pub const fn with_multiproof_chunk_size(mut self, multiproof_chunk_size: usize) -> Self {
        self.multiproof_chunk_size = multiproof_chunk_size;
        self
    }

    /// Setter for the number of reserved CPU cores for any non-reth processes
    pub const fn with_reserved_cpu_cores(mut self, reserved_cpu_cores: usize) -> Self {
        self.reserved_cpu_cores = reserved_cpu_cores;
        self
    }

    /// Setter for whether to disable the precompile cache.
    pub const fn without_precompile_cache(mut self, precompile_cache_disabled: bool) -> Self {
        self.precompile_cache_disabled = precompile_cache_disabled;
        self
    }

    /// Setter for whether to use state root fallback, useful for testing.
    pub const fn with_state_root_fallback(mut self, state_root_fallback: bool) -> Self {
        self.state_root_fallback = state_root_fallback;
        self
    }

    /// Setter for whether to unwind canonical header to ancestor during forkchoice updates.
    pub const fn with_unwind_canonical_header(mut self, unwind_canonical_header: bool) -> Self {
        self.allow_unwind_canonical_header = unwind_canonical_header;
        self
    }

    /// Whether or not to use state root task
    pub const fn use_state_root_task(&self) -> bool {
        self.has_enough_parallelism && !self.legacy_state_root
    }

    /// Returns whether cache metrics recording is disabled.
    pub const fn disable_cache_metrics(&self) -> bool {
        self.disable_cache_metrics
    }

    /// Setter for whether to disable cache metrics recording.
    pub const fn without_cache_metrics(mut self, disable_cache_metrics: bool) -> Self {
        self.disable_cache_metrics = disable_cache_metrics;
        self
    }

    /// Returns the sparse trie prune depth.
    pub const fn sparse_trie_prune_depth(&self) -> usize {
        self.sparse_trie_prune_depth
    }

    /// Setter for sparse trie prune depth.
    pub const fn with_sparse_trie_prune_depth(mut self, depth: usize) -> Self {
        self.sparse_trie_prune_depth = depth;
        self
    }

    /// Returns the LFU hot-slot capacity for sparse trie pruning.
    pub const fn sparse_trie_max_hot_slots(&self) -> usize {
        self.sparse_trie_max_hot_slots
    }

    /// Setter for LFU hot-slot capacity.
    pub const fn with_sparse_trie_max_hot_slots(mut self, max_hot_slots: usize) -> Self {
        self.sparse_trie_max_hot_slots = max_hot_slots;
        self
    }

    /// Returns the LFU hot-account capacity for sparse trie pruning.
    pub const fn sparse_trie_max_hot_accounts(&self) -> usize {
        self.sparse_trie_max_hot_accounts
    }

    /// Setter for LFU hot-account capacity.
    pub const fn with_sparse_trie_max_hot_accounts(mut self, max_hot_accounts: usize) -> Self {
        self.sparse_trie_max_hot_accounts = max_hot_accounts;
        self
    }

    /// Returns the slow block threshold, if configured.
    ///
    /// When `Some`, blocks whose total processing time exceeds this duration emit a structured
    /// warning with timing, state-operation, and cache-hit-rate details. `Duration::ZERO` logs
    /// every block.
    pub const fn slow_block_threshold(&self) -> Option<Duration> {
        self.slow_block_threshold
    }

    /// Setter for slow block threshold.
    pub const fn with_slow_block_threshold(
        mut self,
        slow_block_threshold: Option<Duration>,
    ) -> Self {
        self.slow_block_threshold = slow_block_threshold;
        self
    }

    /// Returns whether sparse trie cache pruning is disabled.
    pub const fn disable_sparse_trie_cache_pruning(&self) -> bool {
        self.disable_sparse_trie_cache_pruning
    }

    /// Setter for whether to disable sparse trie cache pruning.
    pub const fn with_disable_sparse_trie_cache_pruning(mut self, value: bool) -> Self {
        self.disable_sparse_trie_cache_pruning = value;
        self
    }

    /// Returns the state root task timeout.
    pub const fn state_root_task_timeout(&self) -> Option<Duration> {
        self.state_root_task_timeout
    }

    /// Setter for state root task timeout.
    pub const fn with_state_root_task_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.state_root_task_timeout = timeout;
        self
    }

    /// Returns whether to share execution cache with the payload builder.
    pub const fn share_execution_cache_with_payload_builder(&self) -> bool {
        self.share_execution_cache_with_payload_builder
    }

    /// Returns whether to share sparse trie with the payload builder.
    pub const fn share_sparse_trie_with_payload_builder(&self) -> bool {
        self.share_sparse_trie_with_payload_builder
    }

    /// Setter for whether to share execution cache with the payload builder.
    pub const fn with_share_execution_cache_with_payload_builder(
        mut self,
        share_execution_cache_with_payload_builder: bool,
    ) -> Self {
        self.share_execution_cache_with_payload_builder =
            share_execution_cache_with_payload_builder;
        self
    }

    /// Setter for whether to share sparse trie with the payload builder.
    pub const fn with_share_sparse_trie_with_payload_builder(
        mut self,
        share_sparse_trie_with_payload_builder: bool,
    ) -> Self {
        self.share_sparse_trie_with_payload_builder = share_sparse_trie_with_payload_builder;
        self
    }

    /// Returns whether persistence is suppressed during payload building.
    pub const fn suppress_persistence_during_build(&self) -> bool {
        self.suppress_persistence_during_build
    }

    /// Setter for whether to suppress persistence during payload building.
    pub const fn with_suppress_persistence_during_build(mut self, value: bool) -> Self {
        self.suppress_persistence_during_build = value;
        self
    }

    /// Returns whether BAL-based parallel execution is disabled.
    pub const fn disable_bal_parallel_execution(&self) -> bool {
        self.disable_bal_parallel_execution
    }

    /// Setter for whether to disable BAL-based parallel execution.
    pub const fn without_bal_parallel_execution(
        mut self,
        disable_bal_parallel_execution: bool,
    ) -> Self {
        self.disable_bal_parallel_execution = disable_bal_parallel_execution;
        self
    }

    /// Returns whether BAL-driven parallel state root computation is disabled.
    pub const fn disable_bal_parallel_state_root(&self) -> bool {
        self.disable_bal_parallel_state_root
    }

    /// Setter for whether to disable BAL-driven parallel state root computation.
    pub const fn without_bal_parallel_state_root(
        mut self,
        disable_bal_parallel_state_root: bool,
    ) -> Self {
        self.disable_bal_parallel_state_root = disable_bal_parallel_state_root;
        self
    }

    /// Returns whether BAL batched IO is disabled.
    pub const fn disable_bal_batch_io(&self) -> bool {
        self.disable_bal_batch_io
    }

    /// Setter for whether to disable BAL batched IO.
    pub const fn without_bal_batch_io(mut self, disable_bal_batch_io: bool) -> Self {
        self.disable_bal_batch_io = disable_bal_batch_io;
        self
    }

    /// Returns the proof jitter duration, if configured (trie-debug only).
    #[cfg(feature = "trie-debug")]
    pub const fn proof_jitter(&self) -> Option<Duration> {
        self.proof_jitter
    }

    /// Setter for proof jitter (trie-debug only).
    #[cfg(feature = "trie-debug")]
    pub const fn with_proof_jitter(mut self, proof_jitter: Option<Duration>) -> Self {
        self.proof_jitter = proof_jitter;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::TreeConfig;

    #[test]
    #[should_panic(
        expected = "persistence_backpressure_threshold must be greater than persistence_threshold"
    )]
    fn rejects_backpressure_threshold_at_or_below_persistence_threshold() {
        let _ = TreeConfig::default()
            .with_persistence_threshold(4)
            .with_persistence_backpressure_threshold(4);
    }
}
