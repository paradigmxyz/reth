//! clap [Args](clap::Args) for RPC related arguments.

use crate::{
    args::{
        types::{MaxU32, ZeroAsNoneU64},
        GasPriceOracleArgs, RpcStateCacheArgs,
    },
    cli::{
        components::{RethNodeComponents, RethRpcComponents, RethRpcServerHandles},
        config::RethRpcConfig,
        ext::RethNodeCommandConfig,
    },
    utils::get_or_create_jwt_secret_from_path,
};
use clap::{
    builder::{PossibleValue, RangedU64ValueParser, TypedValueParser},
    Arg, Args, Command,
};
use futures::TryFutureExt;
use reth_network_api::{NetworkInfo, Peers};
use reth_payload_builder::EngineTypes;
use reth_provider::{
    AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    EvmEnvProvider, HeaderProvider, StateProviderFactory,
};
use reth_rpc::{
    eth::{cache::EthStateCacheConfig, gas_oracle::GasPriceOracleConfig, RPC_DEFAULT_GAS_CAP},
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
pub(crate) const RPC_DEFAULT_MAX_RESPONSE_SIZE_MB: u32 = 150;
/// Default number of incoming connections.
pub(crate) const RPC_DEFAULT_MAX_CONNECTIONS: u32 = 500;

/// Parameters for configuring the rpc more granularity via CLI
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "RPC")]
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

    /// Hex encoded JWT secret to authenticate the regular RPC server(s), see `--http.api` and
    /// `--ws.api`.
    ///
    /// This is __not__ used for the authenticated engine-API RPC server, see
    /// `--authrpc.jwtsecret`.
    #[arg(long = "rpc.jwtsecret", value_name = "HEX", global = true, required = false)]
    pub rpc_jwtsecret: Option<JwtSecret>,

    /// Set the maximum RPC request payload size for both HTTP and WS in megabytes.
    #[arg(long, default_value_t = RPC_DEFAULT_MAX_REQUEST_SIZE_MB.into())]
    pub rpc_max_request_size: MaxU32,

    /// Set the maximum RPC response payload size for both HTTP and WS in megabytes.
    #[arg(long, visible_alias = "--rpc.returndata.limit", default_value_t = RPC_DEFAULT_MAX_RESPONSE_SIZE_MB.into())]
    pub rpc_max_response_size: MaxU32,

    /// Set the the maximum concurrent subscriptions per connection.
    #[arg(long, default_value_t = RPC_DEFAULT_MAX_SUBS_PER_CONN.into())]
    pub rpc_max_subscriptions_per_connection: MaxU32,

    /// Maximum number of RPC server connections.
    #[arg(long, value_name = "COUNT", default_value_t = RPC_DEFAULT_MAX_CONNECTIONS.into())]
    pub rpc_max_connections: MaxU32,

    /// Maximum number of concurrent tracing requests.
    #[arg(long, value_name = "COUNT", default_value_t = constants::DEFAULT_MAX_TRACING_REQUESTS)]
    pub rpc_max_tracing_requests: u32,

    /// Maximum number of blocks that could be scanned per filter request. (0 = entire chain)
    #[arg(long, value_name = "COUNT", default_value_t = ZeroAsNoneU64::new(constants::DEFAULT_MAX_BLOCKS_PER_FILTER))]
    pub rpc_max_blocks_per_filter: ZeroAsNoneU64,

    /// Maximum number of logs that can be returned in a single response. (0 = no limit)
    #[arg(long, value_name = "COUNT", default_value_t = ZeroAsNoneU64::new(constants::DEFAULT_MAX_LOGS_PER_RESPONSE as u64))]
    pub rpc_max_logs_per_response: ZeroAsNoneU64,

    /// Maximum gas limit for `eth_call` and call tracing RPC methods.
    #[arg(
        long,
        alias = "rpc.gascap",
        value_name = "GAS_CAP",
        value_parser = RangedU64ValueParser::<u64>::new().range(1..),
        default_value_t = RPC_DEFAULT_GAS_CAP.into()
    )]
    pub rpc_gas_cap: u64,

    /// State cache configuration.
    #[clap(flatten)]
    pub rpc_state_cache: RpcStateCacheArgs,

    /// Gas price oracle configuration.
    #[clap(flatten)]
    pub gas_price_oracle: GasPriceOracleArgs,
}

impl RpcServerArgs {
    /// Enables the HTTP-RPC server.
    pub fn with_http(mut self) -> Self {
        self.http = true;
        self
    }

    /// Enables the WS-RPC server.
    pub fn with_ws(mut self) -> Self {
        self.ws = true;
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

        // also adjust the ipc path by appending the instance number to the path used for the
        // endpoint
        self.ipcpath = format!("{}-{}", self.ipcpath, instance);
    }

    /// Configures and launches _all_ servers.
    ///
    /// Returns the handles for the launched regular RPC server(s) (if any) and the server handle
    /// for the auth server that handles the `engine_` API that's accessed by the consensus
    /// layer.
    pub async fn start_servers<Reth, Engine, Conf, Types: EngineTypes>(
        &self,
        components: &Reth,
        engine_api: Engine,
        jwt_secret: JwtSecret,
        conf: &mut Conf,
    ) -> eyre::Result<RethRpcServerHandles>
    where
        Reth: RethNodeComponents,
        Engine: EngineApiServer<Types>,
        Conf: RethNodeCommandConfig,
    {
        let auth_config = self.auth_server_config(jwt_secret)?;

        let module_config = self.transport_rpc_module_config();
        debug!(target: "reth::cli", http=?module_config.http(), ws=?module_config.ws(), "Using RPC module config");

        let (mut modules, auth_module, mut registry) = RpcModuleBuilder::default()
            .with_provider(components.provider())
            .with_pool(components.pool())
            .with_network(components.network())
            .with_events(components.events())
            .with_executor(components.task_executor())
            .build_with_auth_server(module_config, engine_api);

        let rpc_components = RethRpcComponents { registry: &mut registry, modules: &mut modules };
        // apply configured customization
        conf.extend_rpc_modules(self, components, rpc_components)?;

        let server_config = self.rpc_server_config();
        let launch_rpc = modules.clone().start_server(server_config).map_ok(|handle| {
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
        let (rpc, auth) = futures::future::try_join(launch_rpc, launch_auth).await?;
        let handles = RethRpcServerHandles { rpc, auth };

        // call hook
        let rpc_components = RethRpcComponents { registry: &mut registry, modules: &mut modules };
        conf.on_rpc_server_started(self, components, rpc_components, handles.clone())?;

        Ok(handles)
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
            + AccountReader
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
    pub async fn start_auth_server<Provider, Pool, Network, Tasks, Types>(
        &self,
        provider: Provider,
        pool: Pool,
        network: Network,
        executor: Tasks,
        engine_api: EngineApi<Provider, Types>,
        jwt_secret: JwtSecret,
    ) -> Result<AuthServerHandle, RpcError>
    where
        Provider: BlockReaderIdExt
            + ChainSpecProvider
            + EvmEnvProvider
            + HeaderProvider
            + StateProviderFactory
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
        Types: EngineTypes + 'static,
    {
        let socket_address = SocketAddr::new(self.auth_addr, self.auth_port);

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
}

impl RethRpcConfig for RpcServerArgs {
    fn is_ipc_enabled(&self) -> bool {
        // By default IPC is enabled therefor it is enabled if the `ipcdisable` is false.
        !self.ipcdisable
    }

    fn ipc_path(&self) -> &str {
        self.ipcpath.as_str()
    }

    fn eth_config(&self) -> EthConfig {
        EthConfig::default()
            .max_tracing_requests(self.rpc_max_tracing_requests)
            .max_blocks_per_filter(self.rpc_max_blocks_per_filter.unwrap_or_max())
            .max_logs_per_response(self.rpc_max_logs_per_response.unwrap_or_max() as usize)
            .rpc_gas_cap(self.rpc_gas_cap)
            .state_cache(self.state_cache_config())
            .gpo_config(self.gas_price_oracle_config())
    }

    fn state_cache_config(&self) -> EthStateCacheConfig {
        EthStateCacheConfig {
            max_blocks: self.rpc_state_cache.max_blocks,
            max_receipts: self.rpc_state_cache.max_receipts,
            max_envs: self.rpc_state_cache.max_envs,
            max_concurrent_db_requests: self.rpc_state_cache.max_concurrent_db_requests,
        }
    }

    fn rpc_max_request_size_bytes(&self) -> u32 {
        self.rpc_max_request_size.get().saturating_mul(1024 * 1024)
    }

    fn rpc_max_response_size_bytes(&self) -> u32 {
        self.rpc_max_response_size.get().saturating_mul(1024 * 1024)
    }

    fn gas_price_oracle_config(&self) -> GasPriceOracleConfig {
        self.gas_price_oracle.gas_price_oracle_config()
    }

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

    fn http_ws_server_builder(&self) -> ServerBuilder {
        ServerBuilder::new()
            .max_connections(self.rpc_max_connections.get())
            .max_request_body_size(self.rpc_max_request_size_bytes())
            .max_response_body_size(self.rpc_max_response_size_bytes())
            .max_subscriptions_per_connection(self.rpc_max_subscriptions_per_connection.get())
    }

    fn ipc_server_builder(&self) -> IpcServerBuilder {
        IpcServerBuilder::default()
            .max_subscriptions_per_connection(self.rpc_max_subscriptions_per_connection.get())
            .max_request_body_size(self.rpc_max_request_size_bytes())
            .max_response_body_size(self.rpc_max_response_size_bytes())
            .max_connections(self.rpc_max_connections.get())
    }

    fn rpc_server_config(&self) -> RpcServerConfig {
        let mut config = RpcServerConfig::default().with_jwt_secret(self.rpc_secret_key());

        if self.http {
            let socket_address = SocketAddr::new(self.http_addr, self.http_port);
            config = config
                .with_http_address(socket_address)
                .with_http(self.http_ws_server_builder())
                .with_http_cors(self.http_corsdomain.clone())
                .with_ws_cors(self.ws_allowed_origins.clone());
        }

        if self.ws {
            let socket_address = SocketAddr::new(self.ws_addr, self.ws_port);
            config = config.with_ws_address(socket_address).with_ws(self.http_ws_server_builder());
        }

        if self.is_ipc_enabled() {
            config =
                config.with_ipc(self.ipc_server_builder()).with_ipc_endpoint(self.ipcpath.clone());
        }

        config
    }

    fn auth_server_config(&self, jwt_secret: JwtSecret) -> Result<AuthServerConfig, RpcError> {
        let address = SocketAddr::new(self.auth_addr, self.auth_port);

        Ok(AuthServerConfig::builder(jwt_secret).socket_addr(address).build())
    }

    fn auth_jwt_secret(&self, default_jwt_path: PathBuf) -> Result<JwtSecret, JwtError> {
        match self.auth_jwtsecret.as_ref() {
            Some(fpath) => {
                debug!(target: "reth::cli", user_path=?fpath, "Reading JWT auth secret file");
                JwtSecret::from_file(fpath)
            }
            None => get_or_create_jwt_secret_from_path(&default_jwt_path),
        }
    }

    fn rpc_secret_key(&self) -> Option<JwtSecret> {
        self.rpc_jwtsecret.clone()
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
            rpc_jwtsecret: None,
            rpc_max_request_size: RPC_DEFAULT_MAX_REQUEST_SIZE_MB.into(),
            rpc_max_response_size: RPC_DEFAULT_MAX_RESPONSE_SIZE_MB.into(),
            rpc_max_subscriptions_per_connection: RPC_DEFAULT_MAX_SUBS_PER_CONN.into(),
            rpc_max_connections: RPC_DEFAULT_MAX_CONNECTIONS.into(),
            rpc_max_tracing_requests: constants::DEFAULT_MAX_TRACING_REQUESTS,
            rpc_max_blocks_per_filter: constants::DEFAULT_MAX_BLOCKS_PER_FILTER.into(),
            rpc_max_logs_per_response: (constants::DEFAULT_MAX_LOGS_PER_RESPONSE as u64).into(),
            rpc_gas_cap: RPC_DEFAULT_GAS_CAP.into(),
            gas_price_oracle: GasPriceOracleArgs::default(),
            rpc_state_cache: RpcStateCacheArgs::default(),
        }
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
    use reth_rpc_builder::RpcModuleSelection::Selection;
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
    fn test_rpc_server_eth_call_bundle_args() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            "eth,admin,debug,eth-call-bundle",
        ])
        .args;

        let apis = args.http_api.unwrap();
        let expected =
            RpcModuleSelection::try_from_selection(["eth", "admin", "debug", "eth-call-bundle"])
                .unwrap();

        assert_eq!(apis, expected);
    }

    #[test]
    fn test_rpc_server_args_parser_none() {
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth", "--http.api", "none"]).args;
        let apis = args.http_api.unwrap();
        let expected = Selection(vec![]);
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
    fn test_unique_rpc_modules() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            " eth, admin, debug, eth,admin",
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

    #[test]
    fn test_zero_filter_limits() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--rpc-max-blocks-per-filter",
            "0",
            "--rpc-max-logs-per-response",
            "0",
        ])
        .args;

        let config = args.eth_config().filter_config();
        assert_eq!(config.max_blocks_per_filter, Some(u64::MAX));
        assert_eq!(config.max_logs_per_response, Some(usize::MAX));
    }

    #[test]
    fn test_custom_filter_limits() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--rpc-max-blocks-per-filter",
            "100",
            "--rpc-max-logs-per-response",
            "200",
        ])
        .args;

        let config = args.eth_config().filter_config();
        assert_eq!(config.max_blocks_per_filter, Some(100));
        assert_eq!(config.max_logs_per_response, Some(200));
    }

    #[test]
    fn rpc_server_args_default_sanity_test() {
        let default_args = RpcServerArgs::default();
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth"]).args;

        assert_eq!(args, default_args);
    }
}
