//! Engine tree configuration.

const DEFAULT_PERSISTENCE_THRESHOLD: u64 = 256;
const DEFAULT_BLOCK_BUFFER_LIMIT: u32 = 256;
const DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH: u32 = 256;

/// The configuration of the engine tree.
#[derive(Debug)]
pub struct TreeConfig {
    /// Maximum number of blocks to be kept only in memory without triggering persistence.
    persistence_threshold: u64,
    /// Number of pending blocks that cannot be executed due to missing parent and
    /// are kept in cache.
    block_buffer_limit: u32,
    /// Number of invalid headers to keep in cache.
    max_invalid_header_cache_length: u32,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            block_buffer_limit: DEFAULT_BLOCK_BUFFER_LIMIT,
            max_invalid_header_cache_length: DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH,
        }
    }
}

impl TreeConfig {
    /// Create engine tree configuration.
    pub const fn new(
        persistence_threshold: u64,
        block_buffer_limit: u32,
        max_invalid_header_cache_length: u32,
    ) -> Self {
        Self { persistence_threshold, block_buffer_limit, max_invalid_header_cache_length }
    }

    /// Return the persistence threshold.
    pub const fn persistence_threshold(&self) -> u64 {
        self.persistence_threshold
    }

    /// Return the block buffer limit.
    pub const fn block_buffer_limit(&self) -> u32 {
        self.block_buffer_limit
    }

    /// Return the maximum invalid cache header length.
    pub const fn max_invalid_header_cache_length(&self) -> u32 {
        self.max_invalid_header_cache_length
    }

    /// Setter for persistence threshold.
    pub const fn with_persistence_threshold(mut self, persistence_threshold: u64) -> Self {
        self.persistence_threshold = persistence_threshold;
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
}
