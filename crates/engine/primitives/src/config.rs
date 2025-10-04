//! Engine tree configuration.

/// Triggers persistence when the number of canonical blocks in memory exceeds this threshold.
pub const DEFAULT_PERSISTENCE_THRESHOLD: u64 = 2;

/// How close to the canonical head we persist blocks.
pub const DEFAULT_MEMORY_BLOCK_BUFFER_TARGET: u64 = 0;

/// Default maximum concurrency for proof tasks
pub const DEFAULT_MAX_PROOF_TASK_CONCURRENCY: u64 = 256;

/// The size of proof targets chunk to spawn in one multiproof calculation.
pub const DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE: usize = 10;

/// Default number of reserved CPU cores for non-reth processes.
///
/// This will be deducted from the thread count of main reth global threadpool.
pub const DEFAULT_RESERVED_CPU_CORES: usize = 1;

// TODO: Experiment with this + metrics to understand the optimal number
/// Default number of storage proof workers.
pub const DEFAULT_STORAGE_PROOF_WORKERS: usize = 6;

/// Default number of account proof workers.
pub const DEFAULT_ACCOUNT_PROOF_WORKERS: usize = 2;

/// Default maximum concurrency for prewarm task.
pub const DEFAULT_PREWARM_MAX_CONCURRENCY: usize = 16;

const DEFAULT_BLOCK_BUFFER_LIMIT: u32 = 256;
const DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH: u32 = 256;
const DEFAULT_MAX_EXECUTE_BLOCK_BATCH_SIZE: usize = 4;
const DEFAULT_CROSS_BLOCK_CACHE_SIZE: u64 = 4 * 1024 * 1024 * 1024;

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
    /// Number of pending blocks that cannot be executed due to missing parent and
    /// are kept in cache.
    block_buffer_limit: u32,
    /// Number of invalid headers to keep in cache.
    max_invalid_header_cache_length: u32,
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
    /// Whether to disable cross-block caching and parallel prewarming.
    disable_caching_and_prewarming: bool,
    /// Whether to disable the parallel sparse trie state root algorithm.
    disable_parallel_sparse_trie: bool,
    /// Whether to enable state provider metrics.
    state_provider_metrics: bool,
    /// Cross-block cache size in bytes.
    cross_block_cache_size: u64,
    /// Whether the host has enough parallelism to run state root task.
    has_enough_parallelism: bool,
    /// Maximum number of concurrent proof tasks
    max_proof_task_concurrency: u64,
    /// Number of workers dedicated to storage proof execution
    storage_proof_workers: usize,
    /// Number of workers dedicated to account proof execution
    account_proof_workers: usize,
    /// Whether multiproof task should chunk proof targets.
    multiproof_chunking_enabled: bool,
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
    /// Maximum concurrency for the prewarm task.
    prewarm_max_concurrency: usize,
    /// Whether to unwind canonical header to ancestor during forkchoice updates.
    allow_unwind_canonical_header: bool,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            block_buffer_limit: DEFAULT_BLOCK_BUFFER_LIMIT,
            max_invalid_header_cache_length: DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH,
            max_execute_block_batch_size: DEFAULT_MAX_EXECUTE_BLOCK_BATCH_SIZE,
            legacy_state_root: false,
            always_compare_trie_updates: false,
            disable_caching_and_prewarming: false,
            disable_parallel_sparse_trie: false,
            state_provider_metrics: false,
            cross_block_cache_size: DEFAULT_CROSS_BLOCK_CACHE_SIZE,
            has_enough_parallelism: has_enough_parallelism(),
            max_proof_task_concurrency: DEFAULT_MAX_PROOF_TASK_CONCURRENCY,
            storage_proof_workers: DEFAULT_STORAGE_PROOF_WORKERS,
            account_proof_workers: DEFAULT_ACCOUNT_PROOF_WORKERS,
            multiproof_chunking_enabled: true,
            multiproof_chunk_size: DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            precompile_cache_disabled: false,
            state_root_fallback: false,
            always_process_payload_attributes_on_canonical_head: false,
            prewarm_max_concurrency: DEFAULT_PREWARM_MAX_CONCURRENCY,
            allow_unwind_canonical_header: false,
        }
    }
}

impl TreeConfig {
    /// Create engine tree configuration.
    #[expect(clippy::too_many_arguments)]
    pub const fn new(
        persistence_threshold: u64,
        memory_block_buffer_target: u64,
        block_buffer_limit: u32,
        max_invalid_header_cache_length: u32,
        max_execute_block_batch_size: usize,
        legacy_state_root: bool,
        always_compare_trie_updates: bool,
        disable_caching_and_prewarming: bool,
        disable_parallel_sparse_trie: bool,
        state_provider_metrics: bool,
        cross_block_cache_size: u64,
        has_enough_parallelism: bool,
        max_proof_task_concurrency: u64,
        storage_proof_workers: usize,
        account_proof_workers: usize,
        multiproof_chunking_enabled: bool,
        multiproof_chunk_size: usize,
        reserved_cpu_cores: usize,
        precompile_cache_disabled: bool,
        state_root_fallback: bool,
        always_process_payload_attributes_on_canonical_head: bool,
        prewarm_max_concurrency: usize,
        allow_unwind_canonical_header: bool,
    ) -> Self {
        Self {
            persistence_threshold,
            memory_block_buffer_target,
            block_buffer_limit,
            max_invalid_header_cache_length,
            max_execute_block_batch_size,
            legacy_state_root,
            always_compare_trie_updates,
            disable_caching_and_prewarming,
            disable_parallel_sparse_trie,
            state_provider_metrics,
            cross_block_cache_size,
            has_enough_parallelism,
            max_proof_task_concurrency,
            storage_proof_workers,
            account_proof_workers,
            multiproof_chunking_enabled,
            multiproof_chunk_size,
            reserved_cpu_cores,
            precompile_cache_disabled,
            state_root_fallback,
            always_process_payload_attributes_on_canonical_head,
            prewarm_max_concurrency,
            allow_unwind_canonical_header,
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

    /// Return the block buffer limit.
    pub const fn block_buffer_limit(&self) -> u32 {
        self.block_buffer_limit
    }

    /// Return the maximum invalid cache header length.
    pub const fn max_invalid_header_cache_length(&self) -> u32 {
        self.max_invalid_header_cache_length
    }

    /// Return the maximum execute block batch size.
    pub const fn max_execute_block_batch_size(&self) -> usize {
        self.max_execute_block_batch_size
    }

    /// Return the maximum proof task concurrency.
    pub const fn max_proof_task_concurrency(&self) -> u64 {
        self.max_proof_task_concurrency
    }

    /// Return the number of storage proof workers.
    pub const fn storage_proof_workers(&self) -> usize {
        self.storage_proof_workers
    }

    /// Return the number of account proof workers.
    pub const fn account_proof_workers(&self) -> usize {
        self.account_proof_workers
    }

    /// Return whether the multiproof task chunking is enabled.
    pub const fn multiproof_chunking_enabled(&self) -> bool {
        self.multiproof_chunking_enabled
    }

    /// Return the multiproof task chunk size.
    pub const fn multiproof_chunk_size(&self) -> usize {
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

    /// Returns whether or not the parallel sparse trie is disabled.
    pub const fn disable_parallel_sparse_trie(&self) -> bool {
        self.disable_parallel_sparse_trie
    }

    /// Returns whether or not cross-block caching and parallel prewarming should be used.
    pub const fn disable_caching_and_prewarming(&self) -> bool {
        self.disable_caching_and_prewarming
    }

    /// Returns whether to always compare trie updates from the state root task to the trie updates
    /// from the regular state root calculation.
    pub const fn always_compare_trie_updates(&self) -> bool {
        self.always_compare_trie_updates
    }

    /// Returns the cross-block cache size.
    pub const fn cross_block_cache_size(&self) -> u64 {
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

    /// Setter for whether to disable cross-block caching and parallel prewarming.
    pub const fn without_caching_and_prewarming(
        mut self,
        disable_caching_and_prewarming: bool,
    ) -> Self {
        self.disable_caching_and_prewarming = disable_caching_and_prewarming;
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
    pub const fn with_cross_block_cache_size(mut self, cross_block_cache_size: u64) -> Self {
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

    /// Setter for whether to disable the parallel sparse trie
    pub const fn with_disable_parallel_sparse_trie(
        mut self,
        disable_parallel_sparse_trie: bool,
    ) -> Self {
        self.disable_parallel_sparse_trie = disable_parallel_sparse_trie;
        self
    }

    /// Setter for maximum number of concurrent proof tasks.
    pub const fn with_max_proof_task_concurrency(
        mut self,
        max_proof_task_concurrency: u64,
    ) -> Self {
        self.max_proof_task_concurrency = max_proof_task_concurrency;
        self
    }

    /// Setter for number of storage proof workers.
    pub const fn with_storage_proof_workers(mut self, storage_proof_workers: usize) -> Self {
        self.storage_proof_workers = storage_proof_workers;
        self
    }

    /// Setter for number of account proof workers.
    pub const fn with_account_proof_workers(mut self, account_proof_workers: usize) -> Self {
        self.account_proof_workers = account_proof_workers;
        self
    }

    /// Setter for whether multiproof task should chunk proof targets.
    pub const fn with_multiproof_chunking_enabled(
        mut self,
        multiproof_chunking_enabled: bool,
    ) -> Self {
        self.multiproof_chunking_enabled = multiproof_chunking_enabled;
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

    /// Setter for prewarm max concurrency.
    pub const fn with_prewarm_max_concurrency(mut self, prewarm_max_concurrency: usize) -> Self {
        self.prewarm_max_concurrency = prewarm_max_concurrency;
        self
    }

    /// Return the prewarm max concurrency.
    pub const fn prewarm_max_concurrency(&self) -> usize {
        self.prewarm_max_concurrency
    }
}
