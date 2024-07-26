//! Engine tree configuration.

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
            persistence_threshold: 256,
            block_buffer_limit: 256,
            max_invalid_header_cache_length: 256,
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

    /// Return a builder for this type.
    pub fn builder() -> TreeConfigBuilder {
        TreeConfigBuilder::default()
    }
}

/// Engine tree configuration builder.
#[derive(Debug, Default)]
pub struct TreeConfigBuilder {
    persistence_threshold: Option<u64>,
    block_buffer_limit: Option<u32>,
    max_invalid_header_cache_length: Option<u32>,
}

impl TreeConfigBuilder {
    /// Setter for persistence threshold.
    pub const fn with_persistence_threshold(mut self, persistence_threshold: u64) -> Self {
        self.persistence_threshold = Some(persistence_threshold);
        self
    }

    /// Setter for block buffer limit.
    pub const fn with_block_buffer_limit(mut self, block_buffer_limit: u32) -> Self {
        self.block_buffer_limit = Some(block_buffer_limit);
        self
    }

    /// Setter for maximum invalid header cache length.
    pub const fn with_max_invalid_header_cache_length(
        mut self,
        max_invalid_header_cache_length: u32,
    ) -> Self {
        self.max_invalid_header_cache_length = Some(max_invalid_header_cache_length);
        self
    }

    /// Generates the final `TreeConfig`.
    pub fn build(self) -> TreeConfig {
        let mut tree_config = TreeConfig::default();

        if let Some(persistence_threshold) = self.persistence_threshold {
            tree_config.persistence_threshold = persistence_threshold;
        }

        if let Some(block_buffer_limit) = self.block_buffer_limit {
            tree_config.block_buffer_limit = block_buffer_limit;
        }

        if let Some(max_invalid_header_cache_length) = self.max_invalid_header_cache_length {
            tree_config.max_invalid_header_cache_length = max_invalid_header_cache_length;
        }
        tree_config
    }
}
