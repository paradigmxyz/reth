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

/// Number of recent blocks to check for gas price
pub const DEFAULT_GAS_PRICE_BLOCKS: u32 = 20;

/// Gas Price below which the gas price oracle will ignore transactions
pub const DEFAULT_GAS_PRICE_IGNORE: u64 = 2;

/// Maximum transaction priority fee (or gas price before London Fork) to be recommended by the gas
/// price oracle
pub const DEFAULT_GAS_PRICE_MAX: u64 = 500_000_000_000;

/// The percentile of gas prices to use for the estimate
pub const DEFAULT_GAS_PRICE_PERCENTILE: u32 = 60;

/// The default IPC endpoint
#[cfg(windows)]
pub const DEFAULT_IPC_ENDPOINT: &str = r"\\.\pipe\reth.ipc";

/// The default IPC endpoint
#[cfg(not(windows))]
pub const DEFAULT_IPC_ENDPOINT: &str = "/tmp/reth.ipc";
