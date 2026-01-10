//! clap [Args](clap::Args) for engine purposes

use clap::{builder::Resettable, Args};
use reth_engine_primitives::{TreeConfig, DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE};
use std::sync::OnceLock;

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
    memory_block_buffer_target: u64,
    legacy_state_root_task_enabled: bool,
    state_cache_disabled: bool,
    prewarming_disabled: bool,
    parallel_sparse_trie_disabled: bool,
    state_provider_metrics: bool,
    cross_block_cache_size: usize,
    state_root_task_compare_updates: bool,
    accept_execution_requests_hash: bool,
    multiproof_chunking_enabled: bool,
    multiproof_chunk_size: usize,
    reserved_cpu_cores: usize,
    precompile_cache_disabled: bool,
    state_root_fallback: bool,
    always_process_payload_attributes_on_canonical_head: bool,
    allow_unwind_canonical_header: bool,
    storage_worker_count: Option<usize>,
    account_worker_count: Option<usize>,
    enable_proof_v2: bool,
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

    /// Set the default memory block buffer target
    pub const fn with_memory_block_buffer_target(mut self, v: u64) -> Self {
        self.memory_block_buffer_target = v;
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

    /// Set whether to disable parallel sparse trie by default
    pub const fn with_parallel_sparse_trie_disabled(mut self, v: bool) -> Self {
        self.parallel_sparse_trie_disabled = v;
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

    /// Set whether to enable multiproof chunking by default
    pub const fn with_multiproof_chunking_enabled(mut self, v: bool) -> Self {
        self.multiproof_chunking_enabled = v;
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

    /// Set whether to enable proof V2 by default
    pub const fn with_enable_proof_v2(mut self, v: bool) -> Self {
        self.enable_proof_v2 = v;
        self
    }
}

impl Default for DefaultEngineValues {
    fn default() -> Self {
        Self {
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
            legacy_state_root_task_enabled: false,
            state_cache_disabled: false,
            prewarming_disabled: false,
            parallel_sparse_trie_disabled: false,
            state_provider_metrics: false,
            cross_block_cache_size: DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB,
            state_root_task_compare_updates: false,
            accept_execution_requests_hash: false,
            multiproof_chunking_enabled: true,
            multiproof_chunk_size: DEFAULT_MULTIPROOF_TASK_CHUNK_SIZE,
            reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
            precompile_cache_disabled: false,
            state_root_fallback: false,
            always_process_payload_attributes_on_canonical_head: false,
            allow_unwind_canonical_header: false,
            storage_worker_count: None,
            account_worker_count: None,
            enable_proof_v2: false,
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

    /// Configure the target number of blocks to keep in memory.
    #[arg(long = "engine.memory-block-buffer-target", default_value_t = DefaultEngineValues::get_global().memory_block_buffer_target)]
    pub memory_block_buffer_target: u64,

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

    /// CAUTION: This CLI flag has no effect anymore, use --engine.disable-parallel-sparse-trie
    /// if you want to disable usage of the `ParallelSparseTrie`.
    #[deprecated]
    #[arg(long = "engine.parallel-sparse-trie", default_value = "true", hide = true)]
    pub parallel_sparse_trie_enabled: bool,

    /// Disable the parallel sparse trie in the engine.
    #[arg(long = "engine.disable-parallel-sparse-trie", default_value_t = DefaultEngineValues::get_global().parallel_sparse_trie_disabled)]
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

    /// Whether multiproof task should chunk proof targets.
    #[arg(long = "engine.multiproof-chunking", default_value_t = DefaultEngineValues::get_global().multiproof_chunking_enabled)]
    pub multiproof_chunking_enabled: bool,

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
    /// If not specified, defaults to 2x available parallelism, clamped between 2 and 64.
    #[arg(long = "engine.storage-worker-count", default_value = Resettable::from(DefaultEngineValues::get_global().storage_worker_count.map(|v| v.to_string().into())))]
    pub storage_worker_count: Option<usize>,

    /// Configure the number of account proof workers in the Tokio blocking pool.
    /// If not specified, defaults to the same count as storage workers.
    #[arg(long = "engine.account-worker-count", default_value = Resettable::from(DefaultEngineValues::get_global().account_worker_count.map(|v| v.to_string().into())))]
    pub account_worker_count: Option<usize>,

    /// Enable V2 storage proofs for state root calculations
    #[arg(long = "engine.enable-proof-v2", default_value_t = DefaultEngineValues::get_global().enable_proof_v2)]
    pub enable_proof_v2: bool,
}

#[allow(deprecated)]
impl Default for EngineArgs {
    fn default() -> Self {
        let DefaultEngineValues {
            persistence_threshold,
            memory_block_buffer_target,
            legacy_state_root_task_enabled,
            state_cache_disabled,
            prewarming_disabled,
            parallel_sparse_trie_disabled,
            state_provider_metrics,
            cross_block_cache_size,
            state_root_task_compare_updates,
            accept_execution_requests_hash,
            multiproof_chunking_enabled,
            multiproof_chunk_size,
            reserved_cpu_cores,
            precompile_cache_disabled,
            state_root_fallback,
            always_process_payload_attributes_on_canonical_head,
            allow_unwind_canonical_header,
            storage_worker_count,
            account_worker_count,
            enable_proof_v2,
        } = DefaultEngineValues::get_global().clone();
        Self {
            persistence_threshold,
            memory_block_buffer_target,
            legacy_state_root_task_enabled,
            state_root_task_compare_updates,
            caching_and_prewarming_enabled: true,
            state_cache_disabled,
            prewarming_disabled,
            parallel_sparse_trie_enabled: true,
            parallel_sparse_trie_disabled,
            state_provider_metrics,
            cross_block_cache_size,
            accept_execution_requests_hash,
            multiproof_chunking_enabled,
            multiproof_chunk_size,
            reserved_cpu_cores,
            precompile_cache_enabled: true,
            precompile_cache_disabled,
            state_root_fallback,
            always_process_payload_attributes_on_canonical_head,
            allow_unwind_canonical_header,
            storage_worker_count,
            account_worker_count,
            enable_proof_v2,
        }
    }
}

impl EngineArgs {
    /// Creates a [`TreeConfig`] from the engine arguments.
    pub fn tree_config(&self) -> TreeConfig {
        let mut config = TreeConfig::default()
            .with_persistence_threshold(self.persistence_threshold)
            .with_memory_block_buffer_target(self.memory_block_buffer_target)
            .with_legacy_state_root(self.legacy_state_root_task_enabled)
            .without_state_cache(self.state_cache_disabled)
            .without_prewarming(self.prewarming_disabled)
            .with_disable_parallel_sparse_trie(self.parallel_sparse_trie_disabled)
            .with_state_provider_metrics(self.state_provider_metrics)
            .with_always_compare_trie_updates(self.state_root_task_compare_updates)
            .with_cross_block_cache_size(self.cross_block_cache_size * 1024 * 1024)
            .with_multiproof_chunking_enabled(self.multiproof_chunking_enabled)
            .with_multiproof_chunk_size(self.multiproof_chunk_size)
            .with_reserved_cpu_cores(self.reserved_cpu_cores)
            .without_precompile_cache(self.precompile_cache_disabled)
            .with_state_root_fallback(self.state_root_fallback)
            .with_always_process_payload_attributes_on_canonical_head(
                self.always_process_payload_attributes_on_canonical_head,
            )
            .with_unwind_canonical_header(self.allow_unwind_canonical_header);

        if let Some(count) = self.storage_worker_count {
            config = config.with_storage_worker_count(count);
        }

        if let Some(count) = self.account_worker_count {
            config = config.with_account_worker_count(count);
        }

        config = config.with_enable_proof_v2(self.enable_proof_v2);

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
            memory_block_buffer_target: 50,
            legacy_state_root_task_enabled: true,
            caching_and_prewarming_enabled: true,
            state_cache_disabled: true,
            prewarming_disabled: true,
            parallel_sparse_trie_enabled: true,
            parallel_sparse_trie_disabled: true,
            state_provider_metrics: true,
            cross_block_cache_size: 256,
            state_root_task_compare_updates: true,
            accept_execution_requests_hash: true,
            multiproof_chunking_enabled: true,
            multiproof_chunk_size: 512,
            reserved_cpu_cores: 4,
            precompile_cache_enabled: true,
            precompile_cache_disabled: true,
            state_root_fallback: true,
            always_process_payload_attributes_on_canonical_head: true,
            allow_unwind_canonical_header: true,
            storage_worker_count: Some(16),
            account_worker_count: Some(8),
            enable_proof_v2: false,
        };

        let parsed_args = CommandParser::<EngineArgs>::parse_from([
            "reth",
            "--engine.persistence-threshold",
            "100",
            "--engine.memory-block-buffer-target",
            "50",
            "--engine.legacy-state-root",
            "--engine.disable-state-cache",
            "--engine.disable-prewarming",
            "--engine.disable-parallel-sparse-trie",
            "--engine.state-provider-metrics",
            "--engine.cross-block-cache-size",
            "256",
            "--engine.state-root-task-compare-updates",
            "--engine.accept-execution-requests-hash",
            "--engine.multiproof-chunking",
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
        ])
        .args;

        assert_eq!(parsed_args, args);
    }
}
