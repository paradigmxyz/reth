//! clap [Args](clap::Args) for engine purposes

use clap::Args;
use reth_engine_primitives::TreeConfig;

use crate::node_config::{
    DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB, DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
    DEFAULT_PERSISTENCE_THRESHOLD,
};

/// Parameters for configuring the engine driver.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Engine")]
pub struct EngineArgs {
    /// Configure persistence threshold for engine experimental.
    #[arg(long = "engine.persistence-threshold", default_value_t = DEFAULT_PERSISTENCE_THRESHOLD)]
    pub persistence_threshold: u64,

    /// Configure the target number of blocks to keep in memory.
    #[arg(long = "engine.memory-block-buffer-target", default_value_t = DEFAULT_MEMORY_BLOCK_BUFFER_TARGET)]
    pub memory_block_buffer_target: u64,

    /// Enable legacy state root
    #[arg(long = "engine.legacy-state-root", default_value = "false")]
    pub legacy_state_root_task_enabled: bool,

    /// Enable cross-block caching and parallel prewarming
    #[arg(long = "engine.caching-and-prewarming")]
    pub caching_and_prewarming_enabled: bool,

    /// Configure the size of cross-block cache in megabytes
    #[arg(long = "engine.cross-block-cache-size", default_value_t = DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB)]
    pub cross_block_cache_size: u64,

    /// Enable comparing trie updates from the state root task to the trie updates from the regular
    /// state root calculation.
    #[arg(long = "engine.state-root-task-compare-updates")]
    pub state_root_task_compare_updates: bool,

    /// Enables accepting requests hash instead of an array of requests in `engine_newPayloadV4`.
    #[arg(long = "engine.accept-execution-requests-hash")]
    pub accept_execution_requests_hash: bool,
}

impl Default for EngineArgs {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            legacy_state_root_task_enabled: false,
            state_root_task_compare_updates: false,
            caching_and_prewarming_enabled: false,
            cross_block_cache_size: DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB,
            accept_execution_requests_hash: false,
        }
    }
}

impl EngineArgs {
    /// Creates a [`TreeConfig`] from the engine arguments.
    pub fn tree_config(&self) -> TreeConfig {
        TreeConfig::default()
            .with_persistence_threshold(self.persistence_threshold)
            .with_memory_block_buffer_target(self.memory_block_buffer_target)
            .with_legacy_state_root(self.legacy_state_root_task_enabled)
            .with_caching_and_prewarming(self.caching_and_prewarming_enabled)
            .with_always_compare_trie_updates(self.state_root_task_compare_updates)
            .with_cross_block_cache_size(self.cross_block_cache_size * 1024 * 1024)
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
}
