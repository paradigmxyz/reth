//! clap [Args](clap::Args) for engine purposes

use clap::Args;
use reth_engine_primitives::TreeConfig;

use crate::node_config::{
    DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB, DEFAULT_MAX_PROOF_TASK_CONCURRENCY,
    DEFAULT_MEMORY_BLOCK_BUFFER_TARGET, DEFAULT_PERSISTENCE_THRESHOLD, DEFAULT_RESERVED_CPU_CORES,
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

    /// CAUTION: This CLI flag has no effect anymore, use --engine.disable-caching-and-prewarming
    /// if you want to disable caching and prewarming
    #[arg(long = "engine.caching-and-prewarming", default_value = "true", hide = true)]
    #[deprecated]
    pub caching_and_prewarming_enabled: bool,

    /// Disable cross-block caching and parallel prewarming
    #[arg(long = "engine.disable-caching-and-prewarming")]
    pub caching_and_prewarming_disabled: bool,

    /// Enable the parallel sparse trie in the engine.
    #[arg(long = "engine.parallel-sparse-trie", default_value = "false")]
    pub parallel_sparse_trie_enabled: bool,

    /// Enable state provider latency metrics. This allows the engine to collect and report stats
    /// about how long state provider calls took during execution, but this does introduce slight
    /// overhead to state provider calls.
    #[arg(long = "engine.state-provider-metrics", default_value = "false")]
    pub state_provider_metrics: bool,

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

    /// Configure the maximum number of concurrent proof tasks
    #[arg(long = "engine.max-proof-task-concurrency", default_value_t = DEFAULT_MAX_PROOF_TASK_CONCURRENCY)]
    pub max_proof_task_concurrency: u64,

    /// Configure the number of reserved CPU cores for non-reth processes
    #[arg(long = "engine.reserved-cpu-cores", default_value_t = DEFAULT_RESERVED_CPU_CORES)]
    pub reserved_cpu_cores: usize,

    /// CAUTION: This CLI flag has no effect anymore, use --engine.disable-precompile-cache
    /// if you want to disable precompile cache
    #[arg(long = "engine.precompile-cache", default_value = "true", hide = true)]
    #[deprecated]
    pub precompile_cache_enabled: bool,

    /// Disable precompile cache
    #[arg(long = "engine.disable-precompile-cache", default_value = "false")]
    pub precompile_cache_disabled: bool,

    /// Enable state root fallback, useful for testing
    #[arg(long = "engine.state-root-fallback", default_value = "false")]
    pub state_root_fallback: bool,

    /// Always process payload attributes and begin a payload build process even if
    /// `forkchoiceState.headBlockHash` is already the canonical head or an ancestor. See
    /// `TreeConfig::always_process_payload_attributes_on_canonical_head` for more details.
    ///
    /// Note: This is a no-op on OP Stack.
    #[arg(
        long = "engine.always-process-payload-attributes-on-canonical-head",
        default_value = "false"
    )]
    pub always_process_payload_attributes_on_canonical_head: bool,
}

#[allow(deprecated)]
impl Default for EngineArgs {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            legacy_state_root_task_enabled: false,
            state_root_task_compare_updates: false,
            caching_and_prewarming_enabled: true,
            caching_and_prewarming_disabled: false,
            parallel_sparse_trie_enabled: false,
            state_provider_metrics: false,
            cross_block_cache_size: DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB,
            accept_execution_requests_hash: false,
            max_proof_task_concurrency: DEFAULT_MAX_PROOF_TASK_CONCURRENCY,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            precompile_cache_enabled: true,
            precompile_cache_disabled: false,
            state_root_fallback: false,
            always_process_payload_attributes_on_canonical_head: false,
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
            .without_caching_and_prewarming(self.caching_and_prewarming_disabled)
            .with_enable_parallel_sparse_trie(self.parallel_sparse_trie_enabled)
            .with_state_provider_metrics(self.state_provider_metrics)
            .with_always_compare_trie_updates(self.state_root_task_compare_updates)
            .with_cross_block_cache_size(self.cross_block_cache_size * 1024 * 1024)
            .with_max_proof_task_concurrency(self.max_proof_task_concurrency)
            .with_reserved_cpu_cores(self.reserved_cpu_cores)
            .without_precompile_cache(self.precompile_cache_disabled)
            .with_state_root_fallback(self.state_root_fallback)
            .with_always_process_payload_attributes_on_canonical_head(
                self.always_process_payload_attributes_on_canonical_head,
            )
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
