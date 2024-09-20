//! clap [Args](clap::Args) for RPC related arguments.

use std::{
    ffi::OsStr,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};

use alloy_rpc_types_engine::JwtSecret;
use clap::{
    builder::{PossibleValue, RangedU64ValueParser, TypedValueParser},
    Arg, Args, Command,
};
use rand::Rng;
use reth_rpc_server_types::{constants, RethRpcModule, RpcModuleSelection};

use crate::args::{
    types::{MaxU32, ZeroAsNoneU64},
    GasPriceOracleArgs, RpcStateCacheArgs,
};

/// Default max number of subscriptions per connection.
pub(crate) const RPC_DEFAULT_MAX_SUBS_PER_CONN: u32 = 1024;

/// Default max request size in MB.
pub(crate) const RPC_DEFAULT_MAX_REQUEST_SIZE_MB: u32 = 15;

/// Default max response size in MB.
///
/// This is only relevant for very large trace responses.
pub(crate) const RPC_DEFAULT_MAX_RESPONSE_SIZE_MB: u32 = 160;

/// Default number of incoming connections.
pub(crate) const RPC_DEFAULT_MAX_CONNECTIONS: u32 = 500;

/// Parameters for configuring the rpc more granularity via CLI
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "RPC")]
pub struct RpcServerArgs {
    /// Enable the HTTP-RPC server
    #[arg(long, default_value_if("dev", "true", "true"))]
    pub http: bool,

    /// Http server address to listen on
    #[arg(long = "http.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub http_addr: IpAddr,

    /// Http server port to listen on
    #[arg(long = "http.port", default_value_t = constants::DEFAULT_HTTP_RPC_PORT)]
    pub http_port: u16,

    /// Rpc Modules to be configured for the HTTP server
    #[arg(long = "http.api", value_parser = RpcModuleSelectionValueParser::default())]
    pub http_api: Option<RpcModuleSelection>,

    /// Http Corsdomain to allow request from
    #[arg(long = "http.corsdomain")]
    pub http_corsdomain: Option<String>,

    /// Enable the WS-RPC server
    #[arg(long)]
    pub ws: bool,

    /// Ws server address to listen on
    #[arg(long = "ws.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub ws_addr: IpAddr,

    /// Ws server port to listen on
    #[arg(long = "ws.port", default_value_t = constants::DEFAULT_WS_RPC_PORT)]
    pub ws_port: u16,

    /// Origins from which to accept `WebSocket` requests
    #[arg(id = "ws.origins", long = "ws.origins")]
    pub ws_allowed_origins: Option<String>,

    /// Rpc Modules to be configured for the WS server
    #[arg(long = "ws.api", value_parser = RpcModuleSelectionValueParser::default())]
    pub ws_api: Option<RpcModuleSelection>,

    /// Disable the IPC-RPC server
    #[arg(long)]
    pub ipcdisable: bool,

    /// Filename for IPC socket/pipe within the datadir
    #[arg(long, default_value_t = constants::DEFAULT_IPC_ENDPOINT.to_string())]
    pub ipcpath: String,

    /// Auth server address to listen on
    #[arg(long = "authrpc.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub auth_addr: IpAddr,

    /// Auth server port to listen on
    #[arg(long = "authrpc.port", default_value_t = constants::DEFAULT_AUTH_PORT)]
    pub auth_port: u16,

    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
    ///
    /// This will enforce JWT authentication for all requests coming from the consensus layer.
    ///
    /// If no path is provided, a secret will be generated and stored in the datadir under
    /// `<DIR>/<CHAIN_ID>/jwt.hex`. For mainnet this would be `~/.reth/mainnet/jwt.hex` by default.
    #[arg(long = "authrpc.jwtsecret", value_name = "PATH", global = true, required = false)]
    pub auth_jwtsecret: Option<PathBuf>,

    /// Enable auth engine API over IPC
    #[arg(long)]
    pub auth_ipc: bool,

    /// Filename for auth IPC socket/pipe within the datadir
    #[arg(long = "auth-ipc.path", default_value_t = constants::DEFAULT_ENGINE_API_IPC_ENDPOINT.to_string())]
    pub auth_ipc_path: String,

    /// Hex encoded JWT secret to authenticate the regular RPC server(s), see `--http.api` and
    /// `--ws.api`.
    ///
    /// This is __not__ used for the authenticated engine-API RPC server, see
    /// `--authrpc.jwtsecret`.
    #[arg(long = "rpc.jwtsecret", value_name = "HEX", global = true, required = false)]
    pub rpc_jwtsecret: Option<JwtSecret>,

    /// Set the maximum RPC request payload size for both HTTP and WS in megabytes.
    #[arg(long = "rpc.max-request-size", alias = "rpc-max-request-size", default_value_t = RPC_DEFAULT_MAX_REQUEST_SIZE_MB.into())]
    pub rpc_max_request_size: MaxU32,

    /// Set the maximum RPC response payload size for both HTTP and WS in megabytes.
    #[arg(long = "rpc.max-response-size", alias = "rpc-max-response-size", visible_alias = "rpc.returndata.limit", default_value_t = RPC_DEFAULT_MAX_RESPONSE_SIZE_MB.into())]
    pub rpc_max_response_size: MaxU32,

    /// Set the maximum concurrent subscriptions per connection.
    #[arg(long = "rpc.max-subscriptions-per-connection", alias = "rpc-max-subscriptions-per-connection", default_value_t = RPC_DEFAULT_MAX_SUBS_PER_CONN.into())]
    pub rpc_max_subscriptions_per_connection: MaxU32,

    /// Maximum number of RPC server connections.
    #[arg(long = "rpc.max-connections", alias = "rpc-max-connections", value_name = "COUNT", default_value_t = RPC_DEFAULT_MAX_CONNECTIONS.into())]
    pub rpc_max_connections: MaxU32,

    /// Maximum number of concurrent tracing requests.
    #[arg(long = "rpc.max-tracing-requests", alias = "rpc-max-tracing-requests", value_name = "COUNT", default_value_t = constants::default_max_tracing_requests())]
    pub rpc_max_tracing_requests: usize,

    /// Maximum number of blocks that could be scanned per filter request. (0 = entire chain)
    #[arg(long = "rpc.max-blocks-per-filter", alias = "rpc-max-blocks-per-filter", value_name = "COUNT", default_value_t = ZeroAsNoneU64::new(constants::DEFAULT_MAX_BLOCKS_PER_FILTER))]
    pub rpc_max_blocks_per_filter: ZeroAsNoneU64,

    /// Maximum number of logs that can be returned in a single response. (0 = no limit)
    #[arg(long = "rpc.max-logs-per-response", alias = "rpc-max-logs-per-response", value_name = "COUNT", default_value_t = ZeroAsNoneU64::new(constants::DEFAULT_MAX_LOGS_PER_RESPONSE as u64))]
    pub rpc_max_logs_per_response: ZeroAsNoneU64,

    /// Maximum gas limit for `eth_call` and call tracing RPC methods.
    #[arg(
        long = "rpc.gascap",
        alias = "rpc-gascap",
        value_name = "GAS_CAP",
        value_parser = RangedU64ValueParser::<u64>::new().range(1..),
        default_value_t = constants::gas_oracle::RPC_DEFAULT_GAS_CAP
    )]
    pub rpc_gas_cap: u64,

    /// Maximum number of blocks for `eth_simulateV1` call.
    #[arg(
        long = "rpc.max-simulate-blocks",
        value_name = "BLOCKS_COUNT",
        default_value_t = constants::DEFAULT_MAX_SIMULATE_BLOCKS
    )]
    pub rpc_max_simulate_blocks: u64,

    /// The maximum proof window for historical proof generation.
    /// This value allows for generating historical proofs up to
    /// configured number of blocks from current tip (up to `tip - window`).
    #[arg(
        long = "rpc.eth-proof-window",
        default_value_t = constants::DEFAULT_ETH_PROOF_WINDOW,
        value_parser = RangedU64ValueParser::<u64>::new().range(..=constants::MAX_ETH_PROOF_WINDOW)
    )]
    pub rpc_eth_proof_window: u64,

    /// Maximum number of concurrent getproof requests.
    #[arg(long = "rpc.proof-permits", alias = "rpc-proof-permits", value_name = "COUNT", default_value_t = constants::DEFAULT_PROOF_PERMITS)]
    pub rpc_proof_permits: usize,

    /// State cache configuration.
    #[command(flatten)]
    pub rpc_state_cache: RpcStateCacheArgs,

    /// Gas price oracle configuration.
    #[command(flatten)]
    pub gas_price_oracle: GasPriceOracleArgs,
}

impl RpcServerArgs {
    /// Enables the HTTP-RPC server.
    pub const fn with_http(mut self) -> Self {
        self.http = true;
        self
    }

    /// Enables the WS-RPC server.
    pub const fn with_ws(mut self) -> Self {
        self.ws = true;
        self
    }

    /// Enables the Auth IPC
    pub const fn with_auth_ipc(mut self) -> Self {
        self.auth_ipc = true;
        self
    }

    /// Change rpc port numbers based on the instance number.
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
    pub fn adjust_instance_ports(&mut self, instance: u16) {
        debug_assert_ne!(instance, 0, "instance must be non-zero");
        // auth port is scaled by a factor of instance * 100
        self.auth_port += instance * 100 - 100;
        // http port is scaled by a factor of -instance
        self.http_port -= instance - 1;
        // ws port is scaled by a factor of instance * 2
        self.ws_port += instance * 2 - 2;

        // if multiple instances are being run, append the instance number to the ipc path
        if instance > 1 {
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
        let random_string: String = rand::thread_rng()
            .sample_iter(rand::distributions::Alphanumeric)
            .take(8)
            .map(char::from)
            .collect();
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
}

impl Default for RpcServerArgs {
    fn default() -> Self {
        Self {
            http: false,
            http_addr: Ipv4Addr::LOCALHOST.into(),
            http_port: constants::DEFAULT_HTTP_RPC_PORT,
            http_api: None,
            http_corsdomain: None,
            ws: false,
            ws_addr: Ipv4Addr::LOCALHOST.into(),
            ws_port: constants::DEFAULT_WS_RPC_PORT,
            ws_allowed_origins: None,
            ws_api: None,
            ipcdisable: false,
            ipcpath: constants::DEFAULT_IPC_ENDPOINT.to_string(),
            auth_addr: Ipv4Addr::LOCALHOST.into(),
            auth_port: constants::DEFAULT_AUTH_PORT,
            auth_jwtsecret: None,
            auth_ipc: false,
            auth_ipc_path: constants::DEFAULT_ENGINE_API_IPC_ENDPOINT.to_string(),
            rpc_jwtsecret: None,
            rpc_max_request_size: RPC_DEFAULT_MAX_REQUEST_SIZE_MB.into(),
            rpc_max_response_size: RPC_DEFAULT_MAX_RESPONSE_SIZE_MB.into(),
            rpc_max_subscriptions_per_connection: RPC_DEFAULT_MAX_SUBS_PER_CONN.into(),
            rpc_max_connections: RPC_DEFAULT_MAX_CONNECTIONS.into(),
            rpc_max_tracing_requests: constants::default_max_tracing_requests(),
            rpc_max_blocks_per_filter: constants::DEFAULT_MAX_BLOCKS_PER_FILTER.into(),
            rpc_max_logs_per_response: (constants::DEFAULT_MAX_LOGS_PER_RESPONSE as u64).into(),
            rpc_gas_cap: constants::gas_oracle::RPC_DEFAULT_GAS_CAP,
            rpc_max_simulate_blocks: constants::DEFAULT_MAX_SIMULATE_BLOCKS,
            rpc_eth_proof_window: constants::DEFAULT_ETH_PROOF_WINDOW,
            gas_price_oracle: GasPriceOracleArgs::default(),
            rpc_state_cache: RpcStateCacheArgs::default(),
            rpc_proof_permits: constants::DEFAULT_PROOF_PERMITS,
        }
    }
}

/// clap value parser for [`RpcModuleSelection`].
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct RpcModuleSelectionValueParser;

impl TypedValueParser for RpcModuleSelectionValueParser {
    type Value = RpcModuleSelection;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        val.parse::<RpcModuleSelection>().map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = RethRpcModule::all_variant_names().to_vec().join(",");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    [possible values: {possible_values}]"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let values = RethRpcModule::all_variant_names().iter().map(PossibleValue::new);
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
}
