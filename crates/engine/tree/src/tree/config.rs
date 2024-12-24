//! Engine tree configuration.

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
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            block_buffer_limit: DEFAULT_BLOCK_BUFFER_LIMIT,
            max_invalid_header_cache_length: DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH,
            max_execute_block_batch_size: DEFAULT_MAX_EXECUTE_BLOCK_BATCH_SIZE,
        }
    }
}

impl TreeConfig {
    /// Create engine tree configuration.
    pub const fn new(
        persistence_threshold: u64,
        memory_block_buffer_target: u64,
        block_buffer_limit: u32,
        max_invalid_header_cache_length: u32,
        max_execute_block_batch_size: usize,
    ) -> Self {
        Self {
            persistence_threshold,
            memory_block_buffer_target,
            block_buffer_limit,
            max_invalid_header_cache_length,
            max_execute_block_batch_size,
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
}
