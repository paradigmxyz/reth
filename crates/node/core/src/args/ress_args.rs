use clap::Args;

/// The default number of maximum active connections.
const MAX_ACTIVE_CONNECTIONS_DEFAULT: u64 = 5;

/// The default size of witness thread pool.
const WITNESS_THREAD_POOL_SIZE_DEFAULT: usize = 5;

/// The default witness cache size.
const WITNESS_CACHE_SIZE_DEFAULT: u32 = 10;

/// Parameters for configuring the `ress` subprotocol.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Ress")]
pub struct RessArgs {
    /// Enable support for `ress` subprotocol.
    #[arg(long = "ress.enable", default_value_t = false)]
    pub enabled: bool,

    /// The maximum number of active connections for `ress` subprotocol.
    #[arg(long = "ress.max-active-connections", default_value_t = MAX_ACTIVE_CONNECTIONS_DEFAULT)]
    pub max_active_connections: u64,

    /// The number of threads in witness thread pool.
    #[arg(long = "ress.witness-thread-pool-size", default_value_t = WITNESS_THREAD_POOL_SIZE_DEFAULT)]
    pub witness_thread_pool_size: usize,

    /// Witness cache size.
    #[arg(long = "ress.witness-cache-size", default_value_t = WITNESS_CACHE_SIZE_DEFAULT)]
    pub witness_cache_size: u32,
}

impl Default for RessArgs {
    fn default() -> Self {
        Self {
            enabled: false,
            max_active_connections: MAX_ACTIVE_CONNECTIONS_DEFAULT,
            witness_thread_pool_size: WITNESS_THREAD_POOL_SIZE_DEFAULT,
            witness_cache_size: WITNESS_CACHE_SIZE_DEFAULT,
        }
    }
}
