//! clap [Args](clap::Args) for RPC related arguments.

use crate::args::{
    types::{MaxU32, ZeroAsNoneU64},
    GasPriceOracleArgs, RpcStateCacheArgs,
};
use alloy_primitives::map::AddressSet;
use alloy_rpc_types_engine::JwtSecret;
use clap::{
    builder::{PossibleValue, RangedU64ValueParser, Resettable, TypedValueParser},
    Arg, Args, Command,
};
use rand::Rng;
use reth_cli_util::{parse_duration_from_secs_or_ms, parse_ether_value};
use reth_rpc_eth_types::builder::config::PendingBlockKind;
use reth_rpc_server_types::{constants, RethRpcModule, RpcModuleSelection};
use std::{
    ffi::OsStr,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::OnceLock,
    time::Duration,
};
use url::Url;

use super::types::MaxOr;

/// Global static RPC server defaults
static RPC_SERVER_DEFAULTS: OnceLock<DefaultRpcServerArgs> = OnceLock::new();

/// Default max number of subscriptions per connection.
pub(crate) const RPC_DEFAULT_MAX_SUBS_PER_CONN: u32 = 1024;

/// Default max request size in MB.
pub(crate) const RPC_DEFAULT_MAX_REQUEST_SIZE_MB: u32 = 15;

/// Default max response size in MB.
///
/// This is only relevant for very large trace responses.
pub(crate) const RPC_DEFAULT_MAX_RESPONSE_SIZE_MB: u32 = 160;

/// Default number of incoming connections.
///
/// This restricts how many active connections (http, ws) the server accepts.
/// Once exceeded, the server can reject new connections.
pub(crate) const RPC_DEFAULT_MAX_CONNECTIONS: u32 = 500;

/// Default values for RPC server that can be customized
///
/// Global defaults can be set via [`DefaultRpcServerArgs::try_init`].
#[derive(Debug, Clone)]
pub struct DefaultRpcServerArgs {
    http: bool,
    http_addr: IpAddr,
    http_port: u16,
    http_disable_compression: bool,
    http_api: Option<RpcModuleSelection>,
    http_corsdomain: Option<String>,
    ws: bool,
    ws_addr: IpAddr,
    ws_port: u16,
    ws_allowed_origins: Option<String>,
    ws_api: Option<RpcModuleSelection>,
    ipcdisable: bool,
    ipcpath: String,
    ipc_socket_permissions: Option<String>,
    auth_addr: IpAddr,
    auth_port: u16,
    auth_jwtsecret: Option<PathBuf>,
    auth_ipc: bool,
    auth_ipc_path: String,
    disable_auth_server: bool,
    rpc_jwtsecret: Option<JwtSecret>,
    rpc_max_request_size: MaxU32,
    rpc_max_response_size: MaxU32,
    rpc_max_subscriptions_per_connection: MaxU32,
    rpc_max_connections: MaxU32,
    rpc_max_tracing_requests: usize,
    rpc_max_blocking_io_requests: usize,
    rpc_max_trace_filter_blocks: u64,
    rpc_max_blocks_per_filter: ZeroAsNoneU64,
    rpc_max_logs_per_response: ZeroAsNoneU64,
    rpc_gas_cap: u64,
    rpc_evm_memory_limit: u64,
    rpc_tx_fee_cap: u128,
    rpc_max_simulate_blocks: u64,
    rpc_eth_proof_window: u64,
    rpc_proof_permits: usize,
    rpc_pending_block: PendingBlockKind,
    rpc_forwarder: Option<Url>,
    builder_disallow: Option<AddressSet>,
    rpc_state_cache: RpcStateCacheArgs,
    gas_price_oracle: GasPriceOracleArgs,
    rpc_send_raw_transaction_sync_timeout: Duration,
}

impl DefaultRpcServerArgs {
    /// Initialize the global RPC server defaults with this configuration
    pub fn try_init(self) -> Result<(), Self> {
        RPC_SERVER_DEFAULTS.set(self)
    }

    /// Get a reference to the global RPC server defaults
    pub fn get_global() -> &'static Self {
        RPC_SERVER_DEFAULTS.get_or_init(Self::default)
    }

    /// Set the default HTTP enabled state
    pub const fn with_http(mut self, v: bool) -> Self {
        self.http = v;
        self
    }

    /// Set the default HTTP address
    pub const fn with_http_addr(mut self, v: IpAddr) -> Self {
        self.http_addr = v;
        self
    }

    /// Set the default HTTP port
    pub const fn with_http_port(mut self, v: u16) -> Self {
        self.http_port = v;
        self
    }

    /// Set whether to disable HTTP compression by default
    pub const fn with_http_disable_compression(mut self, v: bool) -> Self {
        self.http_disable_compression = v;
        self
    }

    /// Set the default HTTP API modules
    pub fn with_http_api(mut self, v: Option<RpcModuleSelection>) -> Self {
        self.http_api = v;
        self
    }

    /// Set the default HTTP CORS domain
    pub fn with_http_corsdomain(mut self, v: Option<String>) -> Self {
        self.http_corsdomain = v;
        self
    }

    /// Set the default WS enabled state
    pub const fn with_ws(mut self, v: bool) -> Self {
        self.ws = v;
        self
    }

    /// Set the default WS address
    pub const fn with_ws_addr(mut self, v: IpAddr) -> Self {
        self.ws_addr = v;
        self
    }

    /// Set the default WS port
    pub const fn with_ws_port(mut self, v: u16) -> Self {
        self.ws_port = v;
        self
    }

    /// Set the default WS allowed origins
    pub fn with_ws_allowed_origins(mut self, v: Option<String>) -> Self {
        self.ws_allowed_origins = v;
        self
    }

    /// Set the default WS API modules
    pub fn with_ws_api(mut self, v: Option<RpcModuleSelection>) -> Self {
        self.ws_api = v;
        self
    }

    /// Set whether to disable IPC by default
    pub const fn with_ipcdisable(mut self, v: bool) -> Self {
        self.ipcdisable = v;
        self
    }

    /// Set the default IPC path
    pub fn with_ipcpath(mut self, v: String) -> Self {
        self.ipcpath = v;
        self
    }

    /// Set the default IPC socket permissions
    pub fn with_ipc_socket_permissions(mut self, v: Option<String>) -> Self {
        self.ipc_socket_permissions = v;
        self
    }

    /// Set the default auth server address
    pub const fn with_auth_addr(mut self, v: IpAddr) -> Self {
        self.auth_addr = v;
        self
    }

    /// Set the default auth server port
    pub const fn with_auth_port(mut self, v: u16) -> Self {
        self.auth_port = v;
        self
    }

    /// Set the default auth JWT secret path
    pub fn with_auth_jwtsecret(mut self, v: Option<PathBuf>) -> Self {
        self.auth_jwtsecret = v;
        self
    }

    /// Set the default auth IPC enabled state
    pub const fn with_auth_ipc(mut self, v: bool) -> Self {
        self.auth_ipc = v;
        self
    }

    /// Set the default auth IPC path
    pub fn with_auth_ipc_path(mut self, v: String) -> Self {
        self.auth_ipc_path = v;
        self
    }

    /// Set whether to disable the auth server by default
    pub const fn with_disable_auth_server(mut self, v: bool) -> Self {
        self.disable_auth_server = v;
        self
    }

    /// Set the default RPC JWT secret
    pub const fn with_rpc_jwtsecret(mut self, v: Option<JwtSecret>) -> Self {
        self.rpc_jwtsecret = v;
        self
    }

    /// Set the default max request size
    pub const fn with_rpc_max_request_size(mut self, v: MaxU32) -> Self {
        self.rpc_max_request_size = v;
        self
    }

    /// Set the default max response size
    pub const fn with_rpc_max_response_size(mut self, v: MaxU32) -> Self {
        self.rpc_max_response_size = v;
        self
    }

    /// Set the default max subscriptions per connection
    pub const fn with_rpc_max_subscriptions_per_connection(mut self, v: MaxU32) -> Self {
        self.rpc_max_subscriptions_per_connection = v;
        self
    }

    /// Set the default max connections
    pub const fn with_rpc_max_connections(mut self, v: MaxU32) -> Self {
        self.rpc_max_connections = v;
        self
    }

    /// Set the default max tracing requests
    pub const fn with_rpc_max_tracing_requests(mut self, v: usize) -> Self {
        self.rpc_max_tracing_requests = v;
        self
    }

    /// Set the default max blocking IO requests
    pub const fn with_rpc_max_blocking_io_requests(mut self, v: usize) -> Self {
        self.rpc_max_blocking_io_requests = v;
        self
    }

    /// Set the default max trace filter blocks
    pub const fn with_rpc_max_trace_filter_blocks(mut self, v: u64) -> Self {
        self.rpc_max_trace_filter_blocks = v;
        self
    }

    /// Set the default max blocks per filter
    pub const fn with_rpc_max_blocks_per_filter(mut self, v: ZeroAsNoneU64) -> Self {
        self.rpc_max_blocks_per_filter = v;
        self
    }

    /// Set the default max logs per response
    pub const fn with_rpc_max_logs_per_response(mut self, v: ZeroAsNoneU64) -> Self {
        self.rpc_max_logs_per_response = v;
        self
    }

    /// Set the default gas cap
    pub const fn with_rpc_gas_cap(mut self, v: u64) -> Self {
        self.rpc_gas_cap = v;
        self
    }

    /// Set the default EVM memory limit
    pub const fn with_rpc_evm_memory_limit(mut self, v: u64) -> Self {
        self.rpc_evm_memory_limit = v;
        self
    }

    /// Set the default tx fee cap
    pub const fn with_rpc_tx_fee_cap(mut self, v: u128) -> Self {
        self.rpc_tx_fee_cap = v;
        self
    }

    /// Set the default max simulate blocks
    pub const fn with_rpc_max_simulate_blocks(mut self, v: u64) -> Self {
        self.rpc_max_simulate_blocks = v;
        self
    }

    /// Set the default eth proof window
    pub const fn with_rpc_eth_proof_window(mut self, v: u64) -> Self {
        self.rpc_eth_proof_window = v;
        self
    }

    /// Set the default proof permits
    pub const fn with_rpc_proof_permits(mut self, v: usize) -> Self {
        self.rpc_proof_permits = v;
        self
    }

    /// Set the default pending block kind
    pub const fn with_rpc_pending_block(mut self, v: PendingBlockKind) -> Self {
        self.rpc_pending_block = v;
        self
    }

    /// Set the default RPC forwarder
    pub fn with_rpc_forwarder(mut self, v: Option<Url>) -> Self {
        self.rpc_forwarder = v;
        self
    }

    /// Set the default builder disallow addresses
    pub fn with_builder_disallow(mut self, v: Option<AddressSet>) -> Self {
        self.builder_disallow = v;
        self
    }

    /// Set the default RPC state cache args
    pub const fn with_rpc_state_cache(mut self, v: RpcStateCacheArgs) -> Self {
        self.rpc_state_cache = v;
        self
    }

    /// Set the default gas price oracle args
    pub const fn with_gas_price_oracle(mut self, v: GasPriceOracleArgs) -> Self {
        self.gas_price_oracle = v;
        self
    }

    /// Set the default send raw transaction sync timeout
    pub const fn with_rpc_send_raw_transaction_sync_timeout(mut self, v: Duration) -> Self {
        self.rpc_send_raw_transaction_sync_timeout = v;
        self
    }
}

impl Default for DefaultRpcServerArgs {
    fn default() -> Self {
        Self {
            http: false,
            http_addr: Ipv4Addr::LOCALHOST.into(),
            http_port: constants::DEFAULT_HTTP_RPC_PORT,
            http_disable_compression: false,
            http_api: None,
            http_corsdomain: None,
            ws: false,
            ws_addr: Ipv4Addr::LOCALHOST.into(),
            ws_port: constants::DEFAULT_WS_RPC_PORT,
            ws_allowed_origins: None,
            ws_api: None,
            ipcdisable: false,
            ipcpath: constants::DEFAULT_IPC_ENDPOINT.to_string(),
            ipc_socket_permissions: None,
            auth_addr: Ipv4Addr::LOCALHOST.into(),
            auth_port: constants::DEFAULT_AUTH_PORT,
            auth_jwtsecret: None,
            auth_ipc: false,
            auth_ipc_path: constants::DEFAULT_ENGINE_API_IPC_ENDPOINT.to_string(),
            disable_auth_server: false,
            rpc_jwtsecret: None,
            rpc_max_request_size: RPC_DEFAULT_MAX_REQUEST_SIZE_MB.into(),
            rpc_max_response_size: RPC_DEFAULT_MAX_RESPONSE_SIZE_MB.into(),
            rpc_max_subscriptions_per_connection: RPC_DEFAULT_MAX_SUBS_PER_CONN.into(),
            rpc_max_connections: RPC_DEFAULT_MAX_CONNECTIONS.into(),
            rpc_max_tracing_requests: constants::default_max_tracing_requests(),
            rpc_max_blocking_io_requests: constants::DEFAULT_MAX_BLOCKING_IO_REQUEST,
            rpc_max_trace_filter_blocks: constants::DEFAULT_MAX_TRACE_FILTER_BLOCKS,
            rpc_max_blocks_per_filter: constants::DEFAULT_MAX_BLOCKS_PER_FILTER.into(),
            rpc_max_logs_per_response: (constants::DEFAULT_MAX_LOGS_PER_RESPONSE as u64).into(),
            rpc_gas_cap: constants::gas_oracle::RPC_DEFAULT_GAS_CAP,
            rpc_evm_memory_limit: (1 << 32) - 1,
            rpc_tx_fee_cap: constants::DEFAULT_TX_FEE_CAP_WEI,
            rpc_max_simulate_blocks: constants::DEFAULT_MAX_SIMULATE_BLOCKS,
            rpc_eth_proof_window: constants::DEFAULT_ETH_PROOF_WINDOW,
            rpc_proof_permits: constants::DEFAULT_PROOF_PERMITS,
            rpc_pending_block: PendingBlockKind::Full,
            rpc_forwarder: None,
            builder_disallow: None,
            rpc_state_cache: RpcStateCacheArgs::default(),
            gas_price_oracle: GasPriceOracleArgs::default(),
            rpc_send_raw_transaction_sync_timeout:
                constants::RPC_DEFAULT_SEND_RAW_TX_SYNC_TIMEOUT_SECS,
        }
    }
}

/// Parameters for configuring the rpc more granularity via CLI
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "RPC")]
pub struct RpcServerArgs {
    /// Enable the HTTP-RPC server
    #[arg(long, default_value_if("dev", "true", "true"), default_value_t = DefaultRpcServerArgs::get_global().http)]
    pub http: bool,

    /// Http server address to listen on
    #[arg(long = "http.addr", default_value_t = DefaultRpcServerArgs::get_global().http_addr)]
    pub http_addr: IpAddr,

    /// Http server port to listen on
    #[arg(long = "http.port", default_value_t = DefaultRpcServerArgs::get_global().http_port)]
    pub http_port: u16,

    /// Disable compression for HTTP responses
    #[arg(long = "http.disable-compression", default_value_t = DefaultRpcServerArgs::get_global().http_disable_compression)]
    pub http_disable_compression: bool,

    /// Rpc Modules to be configured for the HTTP server
    #[arg(long = "http.api", value_parser = RpcModuleSelectionValueParser::default(), default_value = Resettable::from(DefaultRpcServerArgs::get_global().http_api.as_ref().map(|v| v.to_string().into())))]
    pub http_api: Option<RpcModuleSelection>,

    /// Http Corsdomain to allow request from
    #[arg(long = "http.corsdomain", default_value = Resettable::from(DefaultRpcServerArgs::get_global().http_corsdomain.as_ref().map(|v| v.to_string().into())))]
    pub http_corsdomain: Option<String>,

    /// Enable the WS-RPC server
    #[arg(long, default_value_t = DefaultRpcServerArgs::get_global().ws)]
    pub ws: bool,

    /// Ws server address to listen on
    #[arg(long = "ws.addr", default_value_t = DefaultRpcServerArgs::get_global().ws_addr)]
    pub ws_addr: IpAddr,

    /// Ws server port to listen on
    #[arg(long = "ws.port", default_value_t = DefaultRpcServerArgs::get_global().ws_port)]
    pub ws_port: u16,

    /// Origins from which to accept `WebSocket` requests
    #[arg(id = "ws.origins", long = "ws.origins", alias = "ws.corsdomain", default_value = Resettable::from(DefaultRpcServerArgs::get_global().ws_allowed_origins.as_ref().map(|v| v.to_string().into())))]
    pub ws_allowed_origins: Option<String>,

    /// Rpc Modules to be configured for the WS server
    #[arg(long = "ws.api", value_parser = RpcModuleSelectionValueParser::default(), default_value = Resettable::from(DefaultRpcServerArgs::get_global().ws_api.as_ref().map(|v| v.to_string().into())))]
    pub ws_api: Option<RpcModuleSelection>,

    /// Disable the IPC-RPC server
    #[arg(long, default_value_t = DefaultRpcServerArgs::get_global().ipcdisable)]
    pub ipcdisable: bool,

    /// Filename for IPC socket/pipe within the datadir
    #[arg(long, default_value_t = DefaultRpcServerArgs::get_global().ipcpath.clone())]
    pub ipcpath: String,

    /// Set the permissions for the IPC socket file, in octal format.
    ///
    /// If not specified, the permissions will be set by the system's umask.
    #[arg(long = "ipc.permissions", default_value = Resettable::from(DefaultRpcServerArgs::get_global().ipc_socket_permissions.as_ref().map(|v| v.to_string().into())))]
    pub ipc_socket_permissions: Option<String>,

    /// Auth server address to listen on
    #[arg(long = "authrpc.addr", default_value_t = DefaultRpcServerArgs::get_global().auth_addr)]
    pub auth_addr: IpAddr,

    /// Auth server port to listen on
    #[arg(long = "authrpc.port", default_value_t = DefaultRpcServerArgs::get_global().auth_port)]
    pub auth_port: u16,

    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
    ///
    /// This will enforce JWT authentication for all requests coming from the consensus layer.
    ///
    /// If no path is provided, a secret will be generated and stored in the datadir under
    /// `<DIR>/<CHAIN_ID>/jwt.hex`. For mainnet this would be `~/.local/share/reth/mainnet/jwt.hex`
    /// by default.
    #[arg(long = "authrpc.jwtsecret", value_name = "PATH", global = true, required = false, default_value = Resettable::from(DefaultRpcServerArgs::get_global().auth_jwtsecret.as_ref().map(|v| v.to_string_lossy().into())))]
    pub auth_jwtsecret: Option<PathBuf>,

    /// Enable auth engine API over IPC
    #[arg(long, default_value_t = DefaultRpcServerArgs::get_global().auth_ipc)]
    pub auth_ipc: bool,

    /// Filename for auth IPC socket/pipe within the datadir
    #[arg(long = "auth-ipc.path", default_value_t = DefaultRpcServerArgs::get_global().auth_ipc_path.clone())]
    pub auth_ipc_path: String,

    /// Disable the auth/engine API server.
    ///
    /// This will prevent the authenticated engine-API server from starting. Use this if you're
    /// running a node that doesn't need to serve engine API requests.
    #[arg(long = "disable-auth-server", alias = "disable-engine-api", default_value_t = DefaultRpcServerArgs::get_global().disable_auth_server)]
    pub disable_auth_server: bool,

    /// Hex encoded JWT secret to authenticate the regular RPC server(s), see `--http.api` and
    /// `--ws.api`.
    ///
    /// This is __not__ used for the authenticated engine-API RPC server, see
    /// `--authrpc.jwtsecret`.
    #[arg(long = "rpc.jwtsecret", value_name = "HEX", global = true, required = false, default_value = Resettable::from(DefaultRpcServerArgs::get_global().rpc_jwtsecret.as_ref().map(|v| format!("{:?}", v).into())))]
    pub rpc_jwtsecret: Option<JwtSecret>,

    /// Set the maximum RPC request payload size for both HTTP and WS in megabytes.
    #[arg(long = "rpc.max-request-size", alias = "rpc-max-request-size", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_request_size)]
    pub rpc_max_request_size: MaxU32,

    /// Set the maximum RPC response payload size for both HTTP and WS in megabytes.
    #[arg(long = "rpc.max-response-size", alias = "rpc-max-response-size", visible_alias = "rpc.returndata.limit", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_response_size)]
    pub rpc_max_response_size: MaxU32,

    /// Set the maximum concurrent subscriptions per connection.
    #[arg(long = "rpc.max-subscriptions-per-connection", alias = "rpc-max-subscriptions-per-connection", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_subscriptions_per_connection)]
    pub rpc_max_subscriptions_per_connection: MaxU32,

    /// Maximum number of RPC server connections.
    #[arg(long = "rpc.max-connections", alias = "rpc-max-connections", value_name = "COUNT", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_connections)]
    pub rpc_max_connections: MaxU32,

    /// Maximum number of concurrent tracing requests.
    ///
    /// By default this chooses a sensible value based on the number of available cores.
    /// Tracing requests are generally CPU bound.
    /// Choosing a value that is higher than the available CPU cores can have a negative impact on
    /// the performance of the node and affect the node's ability to maintain sync.
    #[arg(long = "rpc.max-tracing-requests", alias = "rpc-max-tracing-requests", value_name = "COUNT", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_tracing_requests)]
    pub rpc_max_tracing_requests: usize,

    /// Maximum number of concurrent blocking IO requests.
    ///
    /// Blocking IO requests include `eth_call`, `eth_estimateGas`, and similar methods that
    /// require EVM execution. These are spawned as blocking tasks to avoid blocking the async
    /// runtime.
    #[arg(long = "rpc.max-blocking-io-requests", alias = "rpc-max-blocking-io-requests", value_name = "COUNT", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_blocking_io_requests)]
    pub rpc_max_blocking_io_requests: usize,

    /// Maximum number of blocks for `trace_filter` requests.
    #[arg(long = "rpc.max-trace-filter-blocks", alias = "rpc-max-trace-filter-blocks", value_name = "COUNT", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_trace_filter_blocks)]
    pub rpc_max_trace_filter_blocks: u64,

    /// Maximum number of blocks that could be scanned per filter request. (0 = entire chain)
    #[arg(long = "rpc.max-blocks-per-filter", alias = "rpc-max-blocks-per-filter", value_name = "COUNT", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_blocks_per_filter)]
    pub rpc_max_blocks_per_filter: ZeroAsNoneU64,

    /// Maximum number of logs that can be returned in a single response. (0 = no limit)
    #[arg(long = "rpc.max-logs-per-response", alias = "rpc-max-logs-per-response", value_name = "COUNT", default_value_t = DefaultRpcServerArgs::get_global().rpc_max_logs_per_response)]
    pub rpc_max_logs_per_response: ZeroAsNoneU64,

    /// Maximum gas limit for `eth_call` and call tracing RPC methods.
    #[arg(
        long = "rpc.gascap",
        alias = "rpc-gascap",
        value_name = "GAS_CAP",
        value_parser = MaxOr::new(RangedU64ValueParser::<u64>::new().range(1..)),
        default_value_t = DefaultRpcServerArgs::get_global().rpc_gas_cap
    )]
    pub rpc_gas_cap: u64,

    /// Maximum memory the EVM can allocate per RPC request.
    #[arg(
        long = "rpc.evm-memory-limit",
        alias = "rpc-evm-memory-limit",
        value_name = "MEMORY_LIMIT",
        value_parser = MaxOr::new(RangedU64ValueParser::<u64>::new().range(1..)),
        default_value_t = DefaultRpcServerArgs::get_global().rpc_evm_memory_limit
    )]
    pub rpc_evm_memory_limit: u64,

    /// Maximum eth transaction fee (in ether) that can be sent via the RPC APIs (0 = no cap)
    #[arg(
        long = "rpc.txfeecap",
        alias = "rpc-txfeecap",
        value_name = "TX_FEE_CAP",
        value_parser = parse_ether_value,
        default_value = "1.0"
    )]
    pub rpc_tx_fee_cap: u128,

    /// Maximum number of blocks for `eth_simulateV1` call.
    #[arg(
        long = "rpc.max-simulate-blocks",
        value_name = "BLOCKS_COUNT",
        default_value_t = DefaultRpcServerArgs::get_global().rpc_max_simulate_blocks
    )]
    pub rpc_max_simulate_blocks: u64,

    /// The maximum proof window for historical proof generation.
    /// This value allows for generating historical proofs up to
    /// configured number of blocks from current tip (up to `tip - window`).
    #[arg(
        long = "rpc.eth-proof-window",
        default_value_t = DefaultRpcServerArgs::get_global().rpc_eth_proof_window,
        value_parser = RangedU64ValueParser::<u64>::new().range(..=constants::MAX_ETH_PROOF_WINDOW)
    )]
    pub rpc_eth_proof_window: u64,

    /// Maximum number of concurrent getproof requests.
    #[arg(long = "rpc.proof-permits", alias = "rpc-proof-permits", value_name = "COUNT", default_value_t = DefaultRpcServerArgs::get_global().rpc_proof_permits)]
    pub rpc_proof_permits: usize,

    /// Configures the pending block behavior for RPC responses.
    ///
    /// Options: full (include all transactions), empty (header only), none (disable pending
    /// blocks).
    #[arg(long = "rpc.pending-block", default_value = "full", value_name = "KIND")]
    pub rpc_pending_block: PendingBlockKind,

    /// Endpoint to forward transactions to.
    #[arg(long = "rpc.forwarder", alias = "rpc-forwarder", value_name = "FORWARDER")]
    pub rpc_forwarder: Option<Url>,

    /// Path to file containing disallowed addresses, json-encoded list of strings. Block
    /// validation API will reject blocks containing transactions from these addresses.
    #[arg(long = "builder.disallow", value_name = "PATH", value_parser = reth_cli_util::parsers::read_json_from_file::<AddressSet>, default_value = Resettable::from(DefaultRpcServerArgs::get_global().builder_disallow.as_ref().map(|v| format!("{:?}", v).into())))]
    pub builder_disallow: Option<AddressSet>,

    /// State cache configuration.
    #[command(flatten)]
    pub rpc_state_cache: RpcStateCacheArgs,

    /// Gas price oracle configuration.
    #[command(flatten)]
    pub gas_price_oracle: GasPriceOracleArgs,

    /// Timeout for `send_raw_transaction_sync` RPC method.
    #[arg(
        long = "rpc.send-raw-transaction-sync-timeout",
        value_name = "SECONDS",
        default_value = "30s",
        value_parser = parse_duration_from_secs_or_ms,
    )]
    pub rpc_send_raw_transaction_sync_timeout: Duration,

    /// Skip invalid transactions in `testing_buildBlockV1` instead of failing.
    ///
    /// When enabled, transactions that fail execution will be skipped, and all subsequent
    /// transactions from the same sender will also be skipped.
    #[arg(long = "testing.skip-invalid-transactions", default_value_t = true)]
    pub testing_skip_invalid_transactions: bool,

    /// Force upcasting EIP-4844 blob sidecars to EIP-7594 format when Osaka is active.
    ///
    /// When enabled, blob transactions submitted via `eth_sendRawTransaction` with EIP-4844
    /// sidecars will be automatically converted to EIP-7594 format if the next block is Osaka.
    /// By default this is disabled, meaning transactions are submitted as-is.
    #[arg(long = "rpc.force-blob-sidecar-upcasting", default_value_t = false)]
    pub rpc_force_blob_sidecar_upcasting: bool,
}

impl RpcServerArgs {
    /// Enables the HTTP-RPC server.
    pub const fn with_http(mut self) -> Self {
        self.http = true;
        self
    }

    /// Configures modules for the HTTP-RPC server.
    pub fn with_http_api(mut self, http_api: RpcModuleSelection) -> Self {
        self.http_api = Some(http_api);
        self
    }

    /// Enables the WS-RPC server.
    pub const fn with_ws(mut self) -> Self {
        self.ws = true;
        self
    }

    /// Configures modules for WS-RPC server.
    pub fn with_ws_api(mut self, ws_api: RpcModuleSelection) -> Self {
        self.ws_api = Some(ws_api);
        self
    }

    /// Enables the Auth IPC
    pub const fn with_auth_ipc(mut self) -> Self {
        self.auth_ipc = true;
        self
    }

    /// Configures modules for both the HTTP-RPC server and WS-RPC server.
    ///
    /// This is the same as calling both [`Self::with_http_api`] and [`Self::with_ws_api`].
    pub fn with_api(self, api: RpcModuleSelection) -> Self {
        self.with_http_api(api.clone()).with_ws_api(api)
    }

    /// Change rpc port numbers based on the instance number, if provided.
    /// * The `auth_port` is scaled by a factor of `instance * 100`
    /// * The `http_port` is scaled by a factor of `-instance`
    /// * The `ws_port` is scaled by a factor of `instance * 2`
    /// * The `ipcpath` is appended with the instance number: `/tmp/reth.ipc-<instance>`
    ///
    /// # Panics
    /// Warning: if `instance` is zero in debug mode, this will panic.
    ///
    /// This will also panic in debug mode if either:
    /// * `instance` is greater than `655` (scaling would overflow `u16`)
    /// * `self.auth_port / 100 + (instance - 1)` would overflow `u16`
    ///
    /// In release mode, this will silently wrap around.
    pub fn adjust_instance_ports(&mut self, instance: Option<u16>) {
        if let Some(instance) = instance {
            debug_assert_ne!(instance, 0, "instance must be non-zero");
            // auth port is scaled by a factor of instance * 100
            self.auth_port += instance * 100 - 100;
            // http port is scaled by a factor of -instance
            self.http_port -= instance - 1;
            // ws port is scaled by a factor of instance * 2
            self.ws_port += instance * 2 - 2;
            // append instance file to ipc path
            self.ipcpath = format!("{}-{}", self.ipcpath, instance);
        }
    }

    /// Set the http port to zero, to allow the OS to assign a random unused port when the rpc
    /// server binds to a socket.
    pub const fn with_http_unused_port(mut self) -> Self {
        self.http_port = 0;
        self
    }

    /// Set the ws port to zero, to allow the OS to assign a random unused port when the rpc
    /// server binds to a socket.
    pub const fn with_ws_unused_port(mut self) -> Self {
        self.ws_port = 0;
        self
    }

    /// Set the auth port to zero, to allow the OS to assign a random unused port when the rpc
    /// server binds to a socket.
    pub const fn with_auth_unused_port(mut self) -> Self {
        self.auth_port = 0;
        self
    }

    /// Append a random string to the ipc path, to prevent possible collisions when multiple nodes
    /// are being run on the same machine.
    pub fn with_ipc_random_path(mut self) -> Self {
        let random_string: String =
            rand::rng().sample_iter(rand::distr::Alphanumeric).take(8).map(char::from).collect();
        self.ipcpath = format!("{}-{}", self.ipcpath, random_string);
        self
    }

    /// Configure all ports to be set to a random unused port when bound, and set the IPC path to a
    /// random path.
    pub fn with_unused_ports(mut self) -> Self {
        self = self.with_http_unused_port();
        self = self.with_ws_unused_port();
        self = self.with_auth_unused_port();
        self = self.with_ipc_random_path();
        self
    }

    /// Apply a function to the args.
    pub fn apply<F>(self, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        f(self)
    }

    /// Configures the timeout for send raw transaction sync.
    pub const fn with_send_raw_transaction_sync_timeout(mut self, timeout: Duration) -> Self {
        self.rpc_send_raw_transaction_sync_timeout = timeout;
        self
    }

    /// Enables forced blob sidecar upcasting from EIP-4844 to EIP-7594 format.
    pub const fn with_force_blob_sidecar_upcasting(mut self) -> Self {
        self.rpc_force_blob_sidecar_upcasting = true;
        self
    }
}

impl Default for RpcServerArgs {
    fn default() -> Self {
        let DefaultRpcServerArgs {
            http,
            http_addr,
            http_port,
            http_disable_compression,
            http_api,
            http_corsdomain,
            ws,
            ws_addr,
            ws_port,
            ws_allowed_origins,
            ws_api,
            ipcdisable,
            ipcpath,
            ipc_socket_permissions,
            auth_addr,
            auth_port,
            auth_jwtsecret,
            auth_ipc,
            auth_ipc_path,
            disable_auth_server,
            rpc_jwtsecret,
            rpc_max_request_size,
            rpc_max_response_size,
            rpc_max_subscriptions_per_connection,
            rpc_max_connections,
            rpc_max_tracing_requests,
            rpc_max_blocking_io_requests,
            rpc_max_trace_filter_blocks,
            rpc_max_blocks_per_filter,
            rpc_max_logs_per_response,
            rpc_gas_cap,
            rpc_evm_memory_limit,
            rpc_tx_fee_cap,
            rpc_max_simulate_blocks,
            rpc_eth_proof_window,
            rpc_proof_permits,
            rpc_pending_block,
            rpc_forwarder,
            builder_disallow,
            rpc_state_cache,
            gas_price_oracle,
            rpc_send_raw_transaction_sync_timeout,
        } = DefaultRpcServerArgs::get_global().clone();
        Self {
            http,
            http_addr,
            http_port,
            http_disable_compression,
            http_api,
            http_corsdomain,
            ws,
            ws_addr,
            ws_port,
            ws_allowed_origins,
            ws_api,
            ipcdisable,
            ipcpath,
            ipc_socket_permissions,
            auth_addr,
            auth_port,
            auth_jwtsecret,
            auth_ipc,
            auth_ipc_path,
            disable_auth_server,
            rpc_jwtsecret,
            rpc_max_request_size,
            rpc_max_response_size,
            rpc_max_subscriptions_per_connection,
            rpc_max_connections,
            rpc_max_tracing_requests,
            rpc_max_blocking_io_requests,
            rpc_max_trace_filter_blocks,
            rpc_max_blocks_per_filter,
            rpc_max_logs_per_response,
            rpc_gas_cap,
            rpc_evm_memory_limit,
            rpc_tx_fee_cap,
            rpc_max_simulate_blocks,
            rpc_eth_proof_window,
            rpc_proof_permits,
            rpc_pending_block,
            rpc_forwarder,
            builder_disallow,
            rpc_state_cache,
            gas_price_oracle,
            rpc_send_raw_transaction_sync_timeout,
            testing_skip_invalid_transactions: true,
            rpc_force_blob_sidecar_upcasting: false,
        }
    }
}

/// clap value parser for [`RpcModuleSelection`] with configurable validation.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct RpcModuleSelectionValueParser;

impl TypedValueParser for RpcModuleSelectionValueParser {
    type Value = RpcModuleSelection;

    fn parse_ref(
        &self,
        _cmd: &Command,
        _arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        // This will now accept any module name, creating Other(name) for unknowns
        Ok(val
            .parse::<RpcModuleSelection>()
            .expect("RpcModuleSelection parsing cannot fail with Other variant"))
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        // Only show standard modules in help text (excludes "other")
        let values = RethRpcModule::standard_variant_names().map(PossibleValue::new);
        Some(Box::new(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_rpc_server_args_parser() {
        let args =
            CommandParser::<RpcServerArgs>::parse_from(["reth", "--http.api", "eth,admin,debug"])
                .args;

        let apis = args.http_api.unwrap();
        let expected = RpcModuleSelection::try_from_selection(["eth", "admin", "debug"]).unwrap();

        assert_eq!(apis, expected);
    }

    #[test]
    fn test_rpc_server_eth_call_bundle_args() {
        let args =
            CommandParser::<RpcServerArgs>::parse_from(["reth", "--http.api", "eth,admin,debug"])
                .args;

        let apis = args.http_api.unwrap();
        let expected = RpcModuleSelection::try_from_selection(["eth", "admin", "debug"]).unwrap();

        assert_eq!(apis, expected);
    }

    #[test]
    fn test_rpc_server_args_parser_none() {
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth", "--http.api", "none"]).args;
        let apis = args.http_api.unwrap();
        let expected = RpcModuleSelection::Selection(Default::default());
        assert_eq!(apis, expected);
    }

    #[test]
    fn rpc_server_args_default_sanity_test() {
        let default_args = RpcServerArgs::default();
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth"]).args;

        assert_eq!(args, default_args);
    }

    #[test]
    fn test_rpc_tx_fee_cap_parse_integer() {
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth", "--rpc.txfeecap", "2"]).args;
        let expected = 2_000_000_000_000_000_000u128; // 2 ETH in wei
        assert_eq!(args.rpc_tx_fee_cap, expected);
    }

    #[test]
    fn test_rpc_tx_fee_cap_parse_decimal() {
        let args =
            CommandParser::<RpcServerArgs>::parse_from(["reth", "--rpc.txfeecap", "1.5"]).args;
        let expected = 1_500_000_000_000_000_000u128; // 1.5 ETH in wei
        assert_eq!(args.rpc_tx_fee_cap, expected);
    }

    #[test]
    fn test_rpc_tx_fee_cap_parse_zero() {
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth", "--rpc.txfeecap", "0"]).args;
        assert_eq!(args.rpc_tx_fee_cap, 0); // 0 = no cap
    }

    #[test]
    fn test_rpc_tx_fee_cap_parse_none() {
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth"]).args;
        let expected = 1_000_000_000_000_000_000u128;
        assert_eq!(args.rpc_tx_fee_cap, expected); // 1 ETH default cap
    }

    #[test]
    fn test_rpc_server_args() {
        let args = RpcServerArgs {
            http: true,
            http_addr: "127.0.0.1".parse().unwrap(),
            http_port: 8545,
            http_disable_compression: false,
            http_api: Some(RpcModuleSelection::try_from_selection(["eth", "admin"]).unwrap()),
            http_corsdomain: Some("*".to_string()),
            ws: true,
            ws_addr: "127.0.0.1".parse().unwrap(),
            ws_port: 8546,
            ws_allowed_origins: Some("*".to_string()),
            ws_api: Some(RpcModuleSelection::try_from_selection(["eth", "admin"]).unwrap()),
            ipcdisable: false,
            ipcpath: "reth.ipc".to_string(),
            ipc_socket_permissions: Some("0o666".to_string()),
            auth_addr: "127.0.0.1".parse().unwrap(),
            auth_port: 8551,
            auth_jwtsecret: Some(std::path::PathBuf::from("/tmp/jwt.hex")),
            auth_ipc: false,
            auth_ipc_path: "engine.ipc".to_string(),
            disable_auth_server: false,
            rpc_jwtsecret: Some(
                JwtSecret::from_hex(
                    "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
                )
                .unwrap(),
            ),
            rpc_max_request_size: 15u32.into(),
            rpc_max_response_size: 160u32.into(),
            rpc_max_subscriptions_per_connection: 1024u32.into(),
            rpc_max_connections: 500u32.into(),
            rpc_max_tracing_requests: 16,
            rpc_max_blocking_io_requests: 256,
            rpc_max_trace_filter_blocks: 4000,
            rpc_max_blocks_per_filter: 1000u64.into(),
            rpc_max_logs_per_response: 10000u64.into(),
            rpc_gas_cap: 50_000_000,
            rpc_evm_memory_limit: 256,
            rpc_tx_fee_cap: 2_000_000_000_000_000_000u128,
            rpc_max_simulate_blocks: 256,
            rpc_eth_proof_window: 100_000,
            rpc_proof_permits: 16,
            rpc_pending_block: PendingBlockKind::Full,
            rpc_forwarder: Some("http://localhost:8545".parse().unwrap()),
            builder_disallow: None,
            rpc_state_cache: RpcStateCacheArgs {
                max_blocks: 5000,
                max_receipts: 2000,
                max_headers: 1000,
                max_concurrent_db_requests: 512,
                max_cached_tx_hashes: 30_000,
            },
            gas_price_oracle: GasPriceOracleArgs {
                blocks: 20,
                ignore_price: 2,
                max_price: 500_000_000_000,
                percentile: 60,
                default_suggested_fee: None,
            },
            rpc_send_raw_transaction_sync_timeout: std::time::Duration::from_secs(30),
            testing_skip_invalid_transactions: true,
            rpc_force_blob_sidecar_upcasting: false,
        };

        let parsed_args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http",
            "--http.addr",
            "127.0.0.1",
            "--http.port",
            "8545",
            "--http.api",
            "eth,admin",
            "--http.corsdomain",
            "*",
            "--ws",
            "--ws.addr",
            "127.0.0.1",
            "--ws.port",
            "8546",
            "--ws.origins",
            "*",
            "--ws.api",
            "eth,admin",
            "--ipcpath",
            "reth.ipc",
            "--ipc.permissions",
            "0o666",
            "--authrpc.addr",
            "127.0.0.1",
            "--authrpc.port",
            "8551",
            "--authrpc.jwtsecret",
            "/tmp/jwt.hex",
            "--auth-ipc.path",
            "engine.ipc",
            "--rpc.jwtsecret",
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            "--rpc.max-request-size",
            "15",
            "--rpc.max-response-size",
            "160",
            "--rpc.max-subscriptions-per-connection",
            "1024",
            "--rpc.max-connections",
            "500",
            "--rpc.max-tracing-requests",
            "16",
            "--rpc.max-blocking-io-requests",
            "256",
            "--rpc.max-trace-filter-blocks",
            "4000",
            "--rpc.max-blocks-per-filter",
            "1000",
            "--rpc.max-logs-per-response",
            "10000",
            "--rpc.gascap",
            "50000000",
            "--rpc.evm-memory-limit",
            "256",
            "--rpc.txfeecap",
            "2.0",
            "--rpc.max-simulate-blocks",
            "256",
            "--rpc.eth-proof-window",
            "100000",
            "--rpc.proof-permits",
            "16",
            "--rpc.pending-block",
            "full",
            "--rpc.forwarder",
            "http://localhost:8545",
            "--rpc-cache.max-blocks",
            "5000",
            "--rpc-cache.max-receipts",
            "2000",
            "--rpc-cache.max-headers",
            "1000",
            "--rpc-cache.max-concurrent-db-requests",
            "512",
            "--gpo.blocks",
            "20",
            "--gpo.ignoreprice",
            "2",
            "--gpo.maxprice",
            "500000000000",
            "--gpo.percentile",
            "60",
            "--rpc.send-raw-transaction-sync-timeout",
            "30s",
            "--testing.skip-invalid-transactions",
        ])
        .args;

        assert_eq!(parsed_args, args);
    }
}
