//! Engine tree configuration.

/// Triggers persistence when the number of canonical blocks in memory exceeds this threshold.
pub const DEFAULT_PERSISTENCE_THRESHOLD: u64 = 2;

/// How close to the canonical head we persist blocks.
pub const DEFAULT_MEMORY_BLOCK_BUFFER_TARGET: u64 = 2;

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
#[derive(Debug)]
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
    /// new state root task
    legacy_state_root: bool,
    /// Whether to always compare trie updates from the state root task to the trie updates from
    /// the regular state root calculation.
    always_compare_trie_updates: bool,
    /// Whether to use cross-block caching and parallel prewarming
    use_caching_and_prewarming: bool,
    /// Cross-block cache size in bytes.
    cross_block_cache_size: u64,
    /// Whether the host has enough parallelism to run state root task.
    has_enough_parallelism: bool,
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
            use_caching_and_prewarming: false,
            cross_block_cache_size: DEFAULT_CROSS_BLOCK_CACHE_SIZE,
            has_enough_parallelism: has_enough_parallelism(),
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
        use_caching_and_prewarming: bool,
        cross_block_cache_size: u64,
        has_enough_parallelism: bool,
    ) -> Self {
        Self {
            persistence_threshold,
            memory_block_buffer_target,
            block_buffer_limit,
            max_invalid_header_cache_length,
            max_execute_block_batch_size,
            legacy_state_root,
            always_compare_trie_updates,
            use_caching_and_prewarming,
            cross_block_cache_size,
            has_enough_parallelism,
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

    /// Returns whether to use the legacy state root calculation method instead
    /// of the new state root task
    pub const fn legacy_state_root(&self) -> bool {
        self.legacy_state_root
    }

    /// Returns whether or not cross-block caching and parallel prewarming should be used.
    pub const fn use_caching_and_prewarming(&self) -> bool {
        self.use_caching_and_prewarming
    }

    /// Returns whether to always compare trie updates from the state root task to the trie updates
    /// from the regular state root calculation.
    pub const fn always_compare_trie_updates(&self) -> bool {
        self.always_compare_trie_updates
    }

    /// Return the cross-block cache size.
    pub const fn cross_block_cache_size(&self) -> u64 {
        self.cross_block_cache_size
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

    /// Setter for whether to use the new state root task calculation method.
    pub const fn with_caching_and_prewarming(mut self, use_caching_and_prewarming: bool) -> Self {
        self.use_caching_and_prewarming = use_caching_and_prewarming;
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

    /// Whether or not to use state root task
    pub fn use_state_root_task(&self) -> bool {
        self.has_enough_parallelism && !self.legacy_state_root
    }
}
