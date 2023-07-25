//! clap [Args](clap::Args) for RPC related arguments.

use crate::args::GasPriceOracleArgs;
use clap::{
    builder::{PossibleValue, RangedU64ValueParser, TypedValueParser},
    Arg, Args, Command,
};
use futures::TryFutureExt;
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{
    BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader, EvmEnvProvider,
    HeaderProvider, StateProviderFactory,
};
use reth_rpc::{
    eth::{
        cache::{
            DEFAULT_BLOCK_CACHE_MAX_LEN, DEFAULT_ENV_CACHE_MAX_LEN, DEFAULT_RECEIPT_CACHE_MAX_LEN,
        },
        gas_oracle::GasPriceOracleConfig,
        RPC_DEFAULT_GAS_CAP,
    },
    JwtError, JwtSecret,
};
use reth_rpc_builder::{
    auth::{AuthServerConfig, AuthServerHandle},
    constants,
    error::RpcError,
    EthConfig, IpcServerBuilder, RethRpcModule, RpcModuleBuilder, RpcModuleConfig,
    RpcModuleSelection, RpcServerConfig, RpcServerHandle, ServerBuilder, TransportRpcModuleConfig,
};
use reth_rpc_engine_api::{EngineApi, EngineApiServer};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{
    ffi::OsStr,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};
use tracing::{debug, info};

/// Default max number of subscriptions per connection.
pub(crate) const RPC_DEFAULT_MAX_SUBS_PER_CONN: u32 = 1024;
/// Default max request size in MB.
pub(crate) const RPC_DEFAULT_MAX_REQUEST_SIZE_MB: u32 = 15;
/// Default max response size in MB.
///
/// This is only relevant for very large trace responses.
pub(crate) const RPC_DEFAULT_MAX_RESPONSE_SIZE_MB: u32 = 115;
/// Default number of incoming connections.
pub(crate) const RPC_DEFAULT_MAX_CONNECTIONS: u32 = 100;
/// Default number of incoming connections.
pub(crate) const RPC_DEFAULT_MAX_TRACING_REQUESTS: u32 = 25;

/// Parameters for configuring the rpc more granularity via CLI
#[derive(Debug, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "RPC")]
pub struct RpcServerArgs {
    /// Enable the HTTP-RPC server
    #[arg(long, default_value_if("dev", "true", "true"))]
    pub http: bool,

    /// Http server address to listen on
    #[arg(long = "http.addr")]
    pub http_addr: Option<IpAddr>,

    /// Http server port to listen on
    #[arg(long = "http.port")]
    pub http_port: Option<u16>,

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
    #[arg(long = "ws.addr")]
    pub ws_addr: Option<IpAddr>,

    /// Ws server port to listen on
    #[arg(long = "ws.port")]
    pub ws_port: Option<u16>,

    /// Origins from which to accept WebSocket requests
    #[arg(long = "ws.origins", name = "ws.origins")]
    pub ws_allowed_origins: Option<String>,

    /// Rpc Modules to be configured for the WS server
    #[arg(long = "ws.api", value_parser = RpcModuleSelectionValueParser::default())]
    pub ws_api: Option<RpcModuleSelection>,

    /// Disable the IPC-RPC  server
    #[arg(long)]
    pub ipcdisable: bool,

    /// Filename for IPC socket/pipe within the datadir
    #[arg(long)]
    pub ipcpath: Option<String>,

    /// Auth server address to listen on
    #[arg(long = "authrpc.addr")]
    pub auth_addr: Option<IpAddr>,

    /// Auth server port to listen on
    #[arg(long = "authrpc.port")]
    pub auth_port: Option<u16>,

    /// Path to a JWT secret to use for authenticated RPC endpoints
    #[arg(long = "authrpc.jwtsecret", value_name = "PATH", global = true, required = false)]
    pub auth_jwtsecret: Option<PathBuf>,

    /// Set the maximum RPC request payload size for both HTTP and WS in megabytes.
    #[arg(long, default_value_t = RPC_DEFAULT_MAX_REQUEST_SIZE_MB)]
    pub rpc_max_request_size: u32,

    /// Set the maximum RPC response payload size for both HTTP and WS in megabytes.
    #[arg(long, visible_alias = "--rpc.returndata.limit", default_value_t = RPC_DEFAULT_MAX_RESPONSE_SIZE_MB)]
    pub rpc_max_response_size: u32,

    /// Set the the maximum concurrent subscriptions per connection.
    #[arg(long, default_value_t = RPC_DEFAULT_MAX_SUBS_PER_CONN)]
    pub rpc_max_subscriptions_per_connection: u32,

    /// Maximum number of RPC server connections.
    #[arg(long, value_name = "COUNT", default_value_t = RPC_DEFAULT_MAX_CONNECTIONS)]
    pub rpc_max_connections: u32,

    /// Maximum number of concurrent tracing requests.
    #[arg(long, value_name = "COUNT", default_value_t = RPC_DEFAULT_MAX_TRACING_REQUESTS)]
    pub rpc_max_tracing_requests: u32,

    /// Maximum gas limit for `eth_call` and call tracing RPC methods.
    #[arg(
        long,
        alias = "rpc.gascap",
        value_name = "GAS_CAP",
        value_parser = RangedU64ValueParser::<u64>::new().range(1..),
        default_value_t = RPC_DEFAULT_GAS_CAP.into()
    )]
    pub rpc_gas_cap: u64,

    /// Gas price oracle configuration.
    #[clap(flatten)]
    pub gas_price_oracle: GasPriceOracleArgs,

    /// Maximum number of block cache entries.
    #[arg(long, default_value_t = DEFAULT_BLOCK_CACHE_MAX_LEN)]
    pub block_cache_len: u32,

    /// Maximum number of receipt cache entries.
    #[arg(long, default_value_t = DEFAULT_RECEIPT_CACHE_MAX_LEN)]
    pub receipt_cache_len: u32,

    /// Maximum number of env cache entries.
    #[arg(long, default_value_t = DEFAULT_ENV_CACHE_MAX_LEN)]
    pub env_cache_len: u32,
}

impl RpcServerArgs {
    /// Returns the max request size in bytes.
    pub fn rpc_max_request_size_bytes(&self) -> u32 {
        self.rpc_max_request_size * 1024 * 1024
    }

    /// Returns the max response size in bytes.
    pub fn rpc_max_response_size_bytes(&self) -> u32 {
        self.rpc_max_response_size * 1024 * 1024
    }

    /// Extracts the gas price oracle config from the args.
    pub fn gas_price_oracle_config(&self) -> GasPriceOracleConfig {
        GasPriceOracleConfig::new(
            self.gas_price_oracle.blocks,
            self.gas_price_oracle.ignore_price,
            self.gas_price_oracle.max_price,
            self.gas_price_oracle.percentile,
        )
    }

    /// Extracts the [EthConfig] from the args.
    pub fn eth_config(&self) -> EthConfig {
        EthConfig::default()
            .max_tracing_requests(self.rpc_max_tracing_requests)
            .rpc_gas_cap(self.rpc_gas_cap)
            .gpo_config(self.gas_price_oracle_config())
    }

    /// Convenience function that returns whether ipc is enabled
    ///
    /// By default IPC is enabled therefor it is enabled if the `ipcdisable` is false.
    fn is_ipc_enabled(&self) -> bool {
        !self.ipcdisable
    }

    /// The execution layer and consensus layer clients SHOULD accept a configuration parameter:
    /// jwt-secret, which designates a file containing the hex-encoded 256 bit secret key to be used
    /// for verifying/generating JWT tokens.
    ///
    /// If such a parameter is given, but the file cannot be read, or does not contain a hex-encoded
    /// key of 256 bits, the client SHOULD treat this as an error.
    ///
    /// If such a parameter is not given, the client SHOULD generate such a token, valid for the
    /// duration of the execution, and SHOULD store the hex-encoded secret as a jwt.hex file on
    /// the filesystem. This file can then be used to provision the counterpart client.
    ///
    /// The `default_jwt_path` provided as an argument will be used as the default location for the
    /// jwt secret in case the `auth_jwtsecret` argument is not provided.
    pub(crate) fn jwt_secret(&self, default_jwt_path: PathBuf) -> Result<JwtSecret, JwtError> {
        match self.auth_jwtsecret.as_ref() {
            Some(fpath) => {
                debug!(target: "reth::cli", user_path=?fpath, "Reading JWT auth secret file");
                JwtSecret::from_file(fpath)
            }
            None => {
                if default_jwt_path.exists() {
                    debug!(target: "reth::cli", ?default_jwt_path, "Reading JWT auth secret file");
                    JwtSecret::from_file(&default_jwt_path)
                } else {
                    info!(target: "reth::cli", ?default_jwt_path, "Creating JWT auth secret file");
                    JwtSecret::try_create(&default_jwt_path)
                }
            }
        }
    }

    /// Configures and launches _all_ servers.
    ///
    /// Returns the handles for the launched regular RPC server(s) (if any) and the server handle
    /// for the auth server that handles the `engine_` API that's accessed by the consensus
    /// layer.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_servers<Provider, Pool, Network, Tasks, Events, Engine>(
        &self,
        provider: Provider,
        pool: Pool,
        network: Network,
        executor: Tasks,
        events: Events,
        engine_api: Engine,
        jwt_secret: JwtSecret,
    ) -> Result<(RpcServerHandle, AuthServerHandle), RpcError>
    where
        Provider: BlockReaderIdExt
            + HeaderProvider
            + StateProviderFactory
            + EvmEnvProvider
            + ChainSpecProvider
            + ChangeSetReader
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
        Events: CanonStateSubscriptions + Clone + 'static,
        Engine: EngineApiServer,
    {
        let auth_config = self.auth_server_config(jwt_secret)?;

        let module_config = self.transport_rpc_module_config();
        debug!(target: "reth::cli", http=?module_config.http(), ws=?module_config.ws(), "Using RPC module config");

        let (rpc_modules, auth_module) = RpcModuleBuilder::default()
            .with_provider(provider)
            .with_pool(pool)
            .with_network(network)
            .with_events(events)
            .with_executor(executor)
            .build_with_auth_server(module_config, engine_api);

        let server_config = self.rpc_server_config();
        let launch_rpc = rpc_modules.start_server(server_config).map_ok(|handle| {
            if let Some(url) = handle.ipc_endpoint() {
                info!(target: "reth::cli", url=%url, "RPC IPC server started");
            }
            if let Some(addr) = handle.http_local_addr() {
                info!(target: "reth::cli", url=%addr, "RPC HTTP server started");
            }
            if let Some(addr) = handle.ws_local_addr() {
                info!(target: "reth::cli", url=%addr, "RPC WS server started");
            }
            handle
        });

        let launch_auth = auth_module.start_server(auth_config).map_ok(|handle| {
            let addr = handle.local_addr();
            info!(target: "reth::cli", url=%addr, "RPC auth server started");
            handle
        });

        // launch servers concurrently
        futures::future::try_join(launch_rpc, launch_auth).await
    }

    /// Convenience function for starting a rpc server with configs which extracted from cli args.
    pub async fn start_rpc_server<Provider, Pool, Network, Tasks, Events>(
        &self,
        provider: Provider,
        pool: Pool,
        network: Network,
        executor: Tasks,
        events: Events,
    ) -> Result<RpcServerHandle, RpcError>
    where
        Provider: BlockReaderIdExt
            + HeaderProvider
            + StateProviderFactory
            + EvmEnvProvider
            + ChainSpecProvider
            + ChangeSetReader
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
        Events: CanonStateSubscriptions + Clone + 'static,
    {
        reth_rpc_builder::launch(
            provider,
            pool,
            network,
            self.transport_rpc_module_config(),
            self.rpc_server_config(),
            executor,
            events,
        )
        .await
    }

    /// Create Engine API server.
    pub async fn start_auth_server<Provider, Pool, Network, Tasks>(
        &self,
        provider: Provider,
        pool: Pool,
        network: Network,
        executor: Tasks,
        engine_api: EngineApi<Provider>,
        jwt_secret: JwtSecret,
    ) -> Result<AuthServerHandle, RpcError>
    where
        Provider: BlockReaderIdExt
            + HeaderProvider
            + StateProviderFactory
            + EvmEnvProvider
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
    {
        let socket_address = SocketAddr::new(
            self.auth_addr.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            self.auth_port.unwrap_or(constants::DEFAULT_AUTH_PORT),
        );

        reth_rpc_builder::auth::launch(
            provider,
            pool,
            network,
            executor,
            engine_api,
            socket_address,
            jwt_secret,
        )
        .await
    }

    /// Creates the [TransportRpcModuleConfig] from cli args.
    ///
    /// This sets all the api modules, and configures additional settings like gas price oracle
    /// settings in the [TransportRpcModuleConfig].
    fn transport_rpc_module_config(&self) -> TransportRpcModuleConfig {
        let mut config = TransportRpcModuleConfig::default()
            .with_config(RpcModuleConfig::new(self.eth_config()));

        if self.http {
            config = config.with_http(
                self.http_api
                    .clone()
                    .unwrap_or_else(|| RpcModuleSelection::standard_modules().into()),
            );
        }

        if self.ws {
            config = config.with_ws(
                self.ws_api
                    .clone()
                    .unwrap_or_else(|| RpcModuleSelection::standard_modules().into()),
            );
        }

        if self.is_ipc_enabled() {
            config = config.with_ipc(RpcModuleSelection::default_ipc_modules());
        }

        config
    }

    /// Returns the default server builder for http/ws
    fn http_ws_server_builder(&self) -> ServerBuilder {
        ServerBuilder::new()
            .max_connections(self.rpc_max_connections)
            .max_request_body_size(self.rpc_max_request_size_bytes())
            .max_response_body_size(self.rpc_max_response_size_bytes())
            .max_subscriptions_per_connection(self.rpc_max_subscriptions_per_connection)
    }

    /// Returns the default ipc server builder
    fn ipc_server_builder(&self) -> IpcServerBuilder {
        IpcServerBuilder::default()
            .max_subscriptions_per_connection(self.rpc_max_subscriptions_per_connection)
            .max_request_body_size(self.rpc_max_request_size_bytes())
            .max_response_body_size(self.rpc_max_response_size_bytes())
            .max_connections(self.rpc_max_connections)
    }

    /// Creates the [RpcServerConfig] from cli args.
    fn rpc_server_config(&self) -> RpcServerConfig {
        let mut config = RpcServerConfig::default();

        if self.http {
            let socket_address = SocketAddr::new(
                self.http_addr.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                self.http_port.unwrap_or(constants::DEFAULT_HTTP_RPC_PORT),
            );
            config = config
                .with_http_address(socket_address)
                .with_http(self.http_ws_server_builder())
                .with_http_cors(self.http_corsdomain.clone())
                .with_ws_cors(self.ws_allowed_origins.clone());
        }

        if self.ws {
            let socket_address = SocketAddr::new(
                self.ws_addr.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                self.ws_port.unwrap_or(constants::DEFAULT_WS_RPC_PORT),
            );
            config = config.with_ws_address(socket_address).with_ws(self.http_ws_server_builder());
        }

        if self.is_ipc_enabled() {
            config = config.with_ipc(self.ipc_server_builder()).with_ipc_endpoint(
                self.ipcpath.as_ref().unwrap_or(&constants::DEFAULT_IPC_ENDPOINT.to_string()),
            );
        }

        config
    }

    /// Creates the [AuthServerConfig] from cli args.
    fn auth_server_config(&self, jwt_secret: JwtSecret) -> Result<AuthServerConfig, RpcError> {
        let address = SocketAddr::new(
            self.auth_addr.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            self.auth_port.unwrap_or(constants::DEFAULT_AUTH_PORT),
        );

        Ok(AuthServerConfig::builder(jwt_secret).socket_addr(address).build())
    }
}

/// clap value parser for [RpcModuleSelection].
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
            let possible_values = RethRpcModule::all_variants().to_vec().join(",");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    [possible values: {possible_values}]"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let values = RethRpcModule::all_variants().iter().map(PossibleValue::new);
        Some(Box::new(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::net::SocketAddrV4;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn test_rpc_gas_cap() {
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth"]).args;
        let config = args.eth_config();
        assert_eq!(config.rpc_gas_cap, Into::<u64>::into(RPC_DEFAULT_GAS_CAP));

        let args =
            CommandParser::<RpcServerArgs>::parse_from(["reth", "--rpc.gascap", "1000"]).args;
        let config = args.eth_config();
        assert_eq!(config.rpc_gas_cap, 1000);

        let args = CommandParser::<RpcServerArgs>::try_parse_from(["reth", "--rpc.gascap", "0"]);
        assert!(args.is_err());
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
    fn test_transport_rpc_module_config() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            "eth,admin,debug",
            "--http",
            "--ws",
        ])
        .args;
        let config = args.transport_rpc_module_config();
        let expected = vec![RethRpcModule::Eth, RethRpcModule::Admin, RethRpcModule::Debug];
        assert_eq!(config.http().cloned().unwrap().into_selection(), expected);
        assert_eq!(
            config.ws().cloned().unwrap().into_selection(),
            RpcModuleSelection::standard_modules()
        );
    }

    #[test]
    fn test_transport_rpc_module_trim_config() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            " eth, admin, debug",
            "--http",
            "--ws",
        ])
        .args;
        let config = args.transport_rpc_module_config();
        let expected = vec![RethRpcModule::Eth, RethRpcModule::Admin, RethRpcModule::Debug];
        assert_eq!(config.http().cloned().unwrap().into_selection(), expected);
        assert_eq!(
            config.ws().cloned().unwrap().into_selection(),
            RpcModuleSelection::standard_modules()
        );
    }

    #[test]
    fn test_rpc_server_config() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            "eth,admin,debug",
            "--http",
            "--ws",
            "--ws.addr",
            "127.0.0.1",
            "--ws.port",
            "8888",
        ])
        .args;
        let config = args.rpc_server_config();
        assert_eq!(
            config.http_address().unwrap(),
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::LOCALHOST,
                constants::DEFAULT_HTTP_RPC_PORT
            ))
        );
        assert_eq!(
            config.ws_address().unwrap(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8888))
        );
        assert_eq!(config.ipc_endpoint().unwrap().path(), constants::DEFAULT_IPC_ENDPOINT);
    }
}
