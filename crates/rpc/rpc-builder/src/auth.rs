use crate::{
    constants,
    constants::{DEFAULT_MAX_BLOCKS_PER_FILTER, DEFAULT_MAX_LOGS_PER_RESPONSE},
    error::{RpcError, ServerKind},
    EthConfig,
};
use hyper::header::AUTHORIZATION;
pub use jsonrpsee::server::ServerBuilder;
use jsonrpsee::{
    http_client::HeaderMap,
    server::{RpcModule, ServerHandle},
};
use reth_network_api::{NetworkInfo, Peers};
use reth_payload_builder::EngineTypes;
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, HeaderProvider, ReceiptProviderIdExt,
    StateProviderFactory,
};
use reth_rpc::{
    eth::{
        cache::EthStateCache, gas_oracle::GasPriceOracle, EthFilterConfig, FeeHistoryCache,
        FeeHistoryCacheConfig,
    },
    AuthLayer, BlockingTaskPool, Claims, EngineEthApi, EthApi, EthFilter,
    EthSubscriptionIdProvider, JwtAuthValidator, JwtSecret,
};
use reth_rpc_api::{servers::*, EngineApiServer};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Configure and launch a _standalone_ auth server with `engine` and a _new_ `eth` namespace.
#[allow(clippy::too_many_arguments)]
pub async fn launch<Provider, Pool, Network, Tasks, EngineApi, Types>(
    provider: Provider,
    pool: Pool,
    network: Network,
    executor: Tasks,
    engine_api: EngineApi,
    socket_addr: SocketAddr,
    secret: JwtSecret,
) -> Result<AuthServerHandle, RpcError>
where
    Provider: BlockReaderIdExt
        + ChainSpecProvider
        + EvmEnvProvider
        + HeaderProvider
        + ReceiptProviderIdExt
        + StateProviderFactory
        + Clone
        + Unpin
        + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Types: EngineTypes,
    EngineApi: EngineApiServer<Types>,
{
    // spawn a new cache task
    let eth_cache =
        EthStateCache::spawn_with(provider.clone(), Default::default(), executor.clone());

    let gas_oracle = GasPriceOracle::new(provider.clone(), Default::default(), eth_cache.clone());

    let fee_history_cache =
        FeeHistoryCache::new(eth_cache.clone(), FeeHistoryCacheConfig::default());
    let eth_api = EthApi::with_spawner(
        provider.clone(),
        pool.clone(),
        network,
        eth_cache.clone(),
        gas_oracle,
        EthConfig::default().rpc_gas_cap,
        Box::new(executor.clone()),
        BlockingTaskPool::build().expect("failed to build tracing pool"),
        fee_history_cache,
    );
    let config = EthFilterConfig::default()
        .max_logs_per_response(DEFAULT_MAX_LOGS_PER_RESPONSE)
        .max_blocks_per_filter(DEFAULT_MAX_BLOCKS_PER_FILTER);
    let eth_filter =
        EthFilter::new(provider, pool, eth_cache.clone(), config, Box::new(executor.clone()));
    launch_with_eth_api(eth_api, eth_filter, engine_api, socket_addr, secret).await
}

/// Configure and launch a _standalone_ auth server with existing EthApi implementation.
pub async fn launch_with_eth_api<Provider, Pool, Network, EngineApi, Types>(
    eth_api: EthApi<Provider, Pool, Network>,
    eth_filter: EthFilter<Provider, Pool>,
    engine_api: EngineApi,
    socket_addr: SocketAddr,
    secret: JwtSecret,
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
    Types: EngineTypes,
    EngineApi: EngineApiServer<Types>,
{
    // Configure the module and start the server.
    let mut module = RpcModule::new(());
    module.merge(engine_api.into_rpc()).expect("No conflicting methods");
    let engine_eth = EngineEthApi::new(eth_api, eth_filter);
    module.merge(engine_eth.into_rpc()).expect("No conflicting methods");

    // Create auth middleware.
    let middleware =
        tower::ServiceBuilder::new().layer(AuthLayer::new(JwtAuthValidator::new(secret.clone())));

    // By default, both http and ws are enabled.
    let server = ServerBuilder::new()
        .set_middleware(middleware)
        .build(socket_addr)
        .await
        .map_err(|err| RpcError::from_jsonrpsee_error(err, ServerKind::Auth(socket_addr)))?;

    let local_addr = server.local_addr()?;

    let handle = server.start(module);
    Ok(AuthServerHandle { handle, local_addr, secret })
}

/// Server configuration for the auth server.
#[derive(Debug)]
pub struct AuthServerConfig {
    /// Where the server should listen.
    pub(crate) socket_addr: SocketAddr,
    /// The secret for the auth layer of the server.
    pub(crate) secret: JwtSecret,
    /// Configs for JSON-RPC Http.
    pub(crate) server_config: ServerBuilder,
}

// === impl AuthServerConfig ===

impl AuthServerConfig {
    /// Convenience function to create a new `AuthServerConfig`.
    pub fn builder(secret: JwtSecret) -> AuthServerConfigBuilder {
        AuthServerConfigBuilder::new(secret)
    }

    /// Returns the address the server will listen on.
    pub fn address(&self) -> SocketAddr {
        self.socket_addr
    }

    /// Convenience function to start a server in one step.
    pub async fn start(self, module: AuthRpcModule) -> Result<AuthServerHandle, RpcError> {
        let Self { socket_addr, secret, server_config } = self;

        // Create auth middleware.
        let middleware = tower::ServiceBuilder::new()
            .layer(AuthLayer::new(JwtAuthValidator::new(secret.clone())));

        // By default, both http and ws are enabled.
        let server =
            server_config.set_middleware(middleware).build(socket_addr).await.map_err(|err| {
                RpcError::from_jsonrpsee_error(err, ServerKind::Auth(socket_addr))
            })?;

        let local_addr = server.local_addr()?;

        let handle = server.start(module.inner);
        Ok(AuthServerHandle { handle, local_addr, secret })
    }
}

/// Builder type for configuring an `AuthServerConfig`.
#[derive(Debug)]
pub struct AuthServerConfigBuilder {
    socket_addr: Option<SocketAddr>,
    secret: JwtSecret,
    server_config: Option<ServerBuilder>,
}

// === impl AuthServerConfigBuilder ===

impl AuthServerConfigBuilder {
    /// Create a new `AuthServerConfigBuilder` with the given `secret`.
    pub fn new(secret: JwtSecret) -> Self {
        Self { socket_addr: None, secret, server_config: None }
    }

    /// Set the socket address for the server.
    pub fn socket_addr(mut self, socket_addr: SocketAddr) -> Self {
        self.socket_addr = Some(socket_addr);
        self
    }

    /// Set the socket address for the server.
    pub fn maybe_socket_addr(mut self, socket_addr: Option<SocketAddr>) -> Self {
        self.socket_addr = socket_addr;
        self
    }

    /// Set the secret for the server.
    pub fn secret(mut self, secret: JwtSecret) -> Self {
        self.secret = secret;
        self
    }

    /// Configures the JSON-RPC server
    ///
    /// Note: this always configures an [EthSubscriptionIdProvider]
    /// [IdProvider](jsonrpsee::server::IdProvider) for convenience.
    pub fn with_server_config(mut self, config: ServerBuilder) -> Self {
        self.server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Build the `AuthServerConfig`.
    pub fn build(self) -> AuthServerConfig {
        AuthServerConfig {
            socket_addr: self.socket_addr.unwrap_or_else(|| {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), constants::DEFAULT_AUTH_PORT)
            }),
            secret: self.secret,
            server_config: self.server_config.unwrap_or_else(|| {
                ServerBuilder::new()
                    // This needs to large enough to handle large eth_getLogs responses and maximum
                    // payload bodies limit for `engine_getPayloadBodiesByRangeV`
                    // ~750MB per response should be enough
                    .max_response_body_size(750 * 1024 * 1024)
                    // Connections to this server are always authenticated, hence this only affects
                    // connections from the CL or any other client that uses JWT, this should be
                    // more than enough so that the CL (or multiple CL nodes) will never get rate
                    // limited
                    .max_connections(500)
                    // bump the default request size slightly, there aren't any methods exposed with
                    // dynamic request params that can exceed this
                    .max_request_body_size(25 * 1024 * 1024)
                    .set_id_provider(EthSubscriptionIdProvider::default())
            }),
        }
    }
}

/// Holds installed modules for the auth server.
#[derive(Debug)]
pub struct AuthRpcModule {
    pub(crate) inner: RpcModule<()>,
}

// === impl TransportRpcModules ===

impl AuthRpcModule {
    /// Create a new `AuthRpcModule` with the given `engine_api`.
    pub fn new<EngineApi, Types>(engine: EngineApi) -> Self
    where
        Types: EngineTypes,
        EngineApi: EngineApiServer<Types>,
    {
        let mut module = RpcModule::new(());
        module.merge(engine.into_rpc()).expect("No conflicting methods");
        Self { inner: module }
    }

    /// Get a reference to the inner `RpcModule`.
    pub fn module_mut(&mut self) -> &mut RpcModule<()> {
        &mut self.inner
    }

    /// Convenience function for starting a server
    pub async fn start_server(
        self,
        config: AuthServerConfig,
    ) -> Result<AuthServerHandle, RpcError> {
        config.start(self).await
    }
}

/// A handle to the spawned auth server.
///
/// When this type is dropped or [AuthServerHandle::stop] has been called the server will be
/// stopped.
#[derive(Clone, Debug)]
#[must_use = "Server stops if dropped"]
pub struct AuthServerHandle {
    local_addr: SocketAddr,
    handle: ServerHandle,
    secret: JwtSecret,
}

// === impl AuthServerHandle ===

impl AuthServerHandle {
    /// Returns the [`SocketAddr`] of the http server if started.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Tell the server to stop without waiting for the server to stop.
    pub fn stop(self) -> Result<(), RpcError> {
        Ok(self.handle.stop()?)
    }

    /// Returns the url to the http server
    pub fn http_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Returns the url to the ws server
    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.local_addr)
    }

    fn bearer(&self) -> String {
        format!(
            "Bearer {}",
            self.secret
                .encode(&Claims {
                    iat: (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() +
                        Duration::from_secs(60))
                    .as_secs(),
                    exp: None,
                })
                .unwrap()
        )
    }

    /// Returns a http client connected to the server.
    pub fn http_client(&self) -> jsonrpsee::http_client::HttpClient {
        jsonrpsee::http_client::HttpClientBuilder::default()
            .set_headers(HeaderMap::from_iter([(AUTHORIZATION, self.bearer().parse().unwrap())]))
            .build(self.http_url())
            .expect("Failed to create http client")
    }

    /// Returns a ws client connected to the server.
    pub async fn ws_client(&self) -> jsonrpsee::ws_client::WsClient {
        jsonrpsee::ws_client::WsClientBuilder::default()
            .set_headers(HeaderMap::from_iter([(AUTHORIZATION, self.bearer().parse().unwrap())]))
            .build(self.ws_url())
            .await
            .expect("Failed to create ws client")
    }
}
