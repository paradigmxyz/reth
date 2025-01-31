//! Engine tree configuration.

use alloy_eips::merge::EPOCH_SLOTS;

/// The largest gap for which the tree will be used for sync. See docs for `pipeline_run_threshold`
/// for more information.
///
/// This is the default threshold, the distance to the head that the tree will be used for sync.
/// If the distance exceeds this threshold, the pipeline will be used for sync.
pub(crate) const MIN_BLOCKS_FOR_PIPELINE_RUN: u64 = EPOCH_SLOTS;

/// Triggers persistence when the number of canonical blocks in memory exceeds this threshold.
pub const DEFAULT_PERSISTENCE_THRESHOLD: u64 = 2;

/// How close to the canonical head we persist blocks.
pub const DEFAULT_MEMORY_BLOCK_BUFFER_TARGET: u64 = 2;

const DEFAULT_BLOCK_BUFFER_LIMIT: u32 = 256;
const DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH: u32 = 256;

const DEFAULT_MAX_EXECUTE_BLOCK_BATCH_SIZE: usize = 4;

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
    /// Whether to use the new state root task calculation method instead of parallel calculation
    use_state_root_task: bool,
    /// Whether to always compare trie updates from the state root task to the trie updates from
    /// the regular state root calculation.
    always_compare_trie_updates: bool,
    /// Whether to use cross-block caching and parallel prewarming
    use_caching_and_prewarming: bool,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            block_buffer_limit: DEFAULT_BLOCK_BUFFER_LIMIT,
            max_invalid_header_cache_length: DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH,
            max_execute_block_batch_size: DEFAULT_MAX_EXECUTE_BLOCK_BATCH_SIZE,
            use_state_root_task: false,
            always_compare_trie_updates: false,
            use_caching_and_prewarming: false,
        }
    }
}

impl TreeConfig {
    /// Create engine tree configuration.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        persistence_threshold: u64,
        memory_block_buffer_target: u64,
        block_buffer_limit: u32,
        max_invalid_header_cache_length: u32,
        max_execute_block_batch_size: usize,
        use_state_root_task: bool,
        always_compare_trie_updates: bool,
        use_caching_and_prewarming: bool,
    ) -> Self {
        Self {
            persistence_threshold,
            memory_block_buffer_target,
            block_buffer_limit,
            max_invalid_header_cache_length,
            max_execute_block_batch_size,
            use_state_root_task,
            always_compare_trie_updates,
            use_caching_and_prewarming,
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

    /// Returns whether to use the state root task calculation method.
    pub const fn use_state_root_task(&self) -> bool {
        self.use_state_root_task
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

    /// Setter for whether to use the new state root task calculation method.
    pub const fn with_state_root_task(mut self, use_state_root_task: bool) -> Self {
        self.use_state_root_task = use_state_root_task;
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
}
