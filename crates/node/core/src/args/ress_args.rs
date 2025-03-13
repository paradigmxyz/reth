use clap::Args;

/// The default number of maximum active connections.
const MAX_ACTIVE_CONNECTIONS_DEFAULT: u64 = 5;

/// The default maximum witness lookback window.
const MAX_WITNESS_WINDOW_DEFAULT: u64 = 1024;

/// The default maximum number of witnesses to generate in parallel.
const WITNESS_MAX_PARALLEL_DEFAULT: usize = 5;

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

    /// The maximum witness lookback window.
    #[arg(long = "ress.max-witness-window", default_value_t = MAX_WITNESS_WINDOW_DEFAULT)]
    pub max_witness_window: u64,

    /// The maximum number of witnesses to generate in parallel.
    #[arg(long = "ress.witness-max-parallel", default_value_t = WITNESS_MAX_PARALLEL_DEFAULT)]
    pub witness_max_parallel: usize,

    /// Witness cache size.
    #[arg(long = "ress.witness-cache-size", default_value_t = WITNESS_CACHE_SIZE_DEFAULT)]
    pub witness_cache_size: u32,
}

impl Default for RessArgs {
    fn default() -> Self {
        Self {
            enabled: false,
            max_active_connections: MAX_ACTIVE_CONNECTIONS_DEFAULT,
            max_witness_window: MAX_WITNESS_WINDOW_DEFAULT,
            witness_max_parallel: WITNESS_MAX_PARALLEL_DEFAULT,
            witness_cache_size: WITNESS_CACHE_SIZE_DEFAULT,
        }
    }
}
