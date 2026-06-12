//! clap [Args](clap::Args) for revmc JIT configuration.

use clap::Args;
use humantime::parse_duration;
use std::time::Duration;

/// Parameters for JIT compilation of EVM bytecode via revmc.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "JIT")]
pub struct JitArgs {
    /// Enable JIT compilation of EVM bytecode.
    #[arg(long = "jit", default_value_t = false, help_heading = "JIT")]
    pub enabled: bool,

    /// Number of observed misses before a bytecode is promoted to JIT compilation.
    #[arg(long = "jit.hot-threshold", default_value_t = Self::DEFAULT_HOT_THRESHOLD, help_heading = "JIT")]
    pub hot_threshold: usize,

    /// Number of JIT compilation worker threads.
    #[arg(long = "jit.worker-count", help_heading = "JIT")]
    pub worker_count: Option<usize>,

    /// Capacity of the lookup-observed event channel.
    /// Events are silently dropped when the channel is full.
    #[arg(long = "jit.channel-capacity", default_value_t = Self::DEFAULT_CHANNEL_CAPACITY, help_heading = "JIT")]
    pub channel_capacity: usize,

    /// Maximum number of pending JIT compilation jobs.
    #[arg(long = "jit.max-pending-jobs", default_value_t = Self::DEFAULT_MAX_PENDING_JOBS, help_heading = "JIT")]
    pub max_pending_jobs: usize,

    /// Maximum bytecode length eligible for JIT compilation.
    /// Contracts with bytecode larger than this are never promoted to JIT.
    /// 0 means no limit.
    #[arg(long = "jit.max-bytecode-len", default_value_t = Self::DEFAULT_MAX_BYTECODE_LEN, help_heading = "JIT")]
    pub max_bytecode_len: usize,

    /// Maximum total resident compiled code size in bytes.
    /// When exceeded, the backend evicts least-recently-used entries.
    /// 0 means no limit.
    #[arg(long = "jit.code-cache-bytes", default_value_t = Self::DEFAULT_CODE_CACHE_BYTES, help_heading = "JIT")]
    pub code_cache_bytes: usize,

    /// Duration after which a compiled program with no lookup hits is evicted.
    #[arg(
        long = "jit.idle-evict-duration",
        default_value = humantime::format_duration(Self::DEFAULT_IDLE_EVICT_DURATION).to_string(),
        help_heading = "JIT",
        value_parser = parse_duration,
    )]
    pub idle_evict_duration: Duration,

    /// Enable compiler debug dumps.
    ///
    /// IR, assembly, and bytecode are written to `<datadir>/jit/<spec_id>/<code_hash>/` for each
    /// compiled contract.
    /// Note that this is not ever cleaned up, and has a non negligible performance overhead.
    #[arg(long = "jit.debug", default_value_t = false, help_heading = "JIT")]
    pub debug: bool,

    /// Blocking mode: synchronously JIT-compile every contract on first encounter.
    /// Intended for debugging only.
    #[doc(hidden)]
    #[arg(long = "jit.blocking", default_value_t = false, help_heading = "JIT", hide = true)]
    pub blocking: bool,
}

impl JitArgs {
    const DEFAULT_HOT_THRESHOLD: usize = 8;
    const DEFAULT_CHANNEL_CAPACITY: usize = 4096;
    const DEFAULT_MAX_PENDING_JOBS: usize = 2048;
    const DEFAULT_MAX_BYTECODE_LEN: usize = 0;
    const DEFAULT_CODE_CACHE_BYTES: usize = 1024 * 1024 * 1024; // 1 GiB
    const DEFAULT_IDLE_EVICT_DURATION: Duration = Duration::from_hours(1);
}

impl Default for JitArgs {
    fn default() -> Self {
        Self {
            enabled: false,
            hot_threshold: Self::DEFAULT_HOT_THRESHOLD,
            worker_count: None,
            channel_capacity: Self::DEFAULT_CHANNEL_CAPACITY,
            max_pending_jobs: Self::DEFAULT_MAX_PENDING_JOBS,
            max_bytecode_len: Self::DEFAULT_MAX_BYTECODE_LEN,
            code_cache_bytes: Self::DEFAULT_CODE_CACHE_BYTES,
            idle_evict_duration: Self::DEFAULT_IDLE_EVICT_DURATION,
            debug: false,
            blocking: false,
        }
    }
}
