/// GPO reexports
pub use reth_rpc::eth::gas_oracle::{
    DEFAULT_GAS_PRICE_BLOCKS, DEFAULT_GAS_PRICE_PERCENTILE, DEFAULT_IGNORE_GAS_PRICE,
    DEFAULT_MAX_GAS_PRICE,
};

/// The default port for the http server
pub const DEFAULT_HTTP_RPC_PORT: u16 = 8545;

/// The default port for the ws server
pub const DEFAULT_WS_RPC_PORT: u16 = 8546;

/// The default port for the auth server.
pub const DEFAULT_AUTH_PORT: u16 = 8551;

/// The default maximum block range allowed to filter
pub const DEFAULT_MAX_BLOCKS_PER_FILTER: u64 = 100_000;

/// The default maximum of logs in a single response.
pub const DEFAULT_MAX_LOGS_PER_RESPONSE: usize = 20_000;

/// The default maximum number of concurrently executed tracing calls
pub const DEFAULT_MAX_TRACING_REQUESTS: u32 = 25;

/// The default IPC endpoint
#[cfg(windows)]
pub const DEFAULT_IPC_ENDPOINT: &str = r"\\.\pipe\reth.ipc";

/// The default IPC endpoint
#[cfg(not(windows))]
pub const DEFAULT_IPC_ENDPOINT: &str = "/tmp/reth.ipc";
