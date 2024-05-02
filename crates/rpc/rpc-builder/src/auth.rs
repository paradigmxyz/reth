use crate::{
    constants,
    constants::{DEFAULT_MAX_BLOCKS_PER_FILTER, DEFAULT_MAX_LOGS_PER_RESPONSE},
    error::{RpcError, ServerKind},
    EthConfig,
};

use hyper::header::AUTHORIZATION;
pub use jsonrpsee::server::ServerBuilder;
use jsonrpsee::{
    core::RegisterMethodError,
    http_client::HeaderMap,
    server::{AlreadyStoppedError, RpcModule},
    Methods,
};
pub use reth_ipc::server::Builder as IpcServerBuilder;

use jsonrpsee::http_client::transport::HttpBackend;
use reth_engine_primitives::EngineTypes;
use reth_evm::ConfigureEvm;
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, HeaderProvider, ReceiptProviderIdExt,
    StateProviderFactory,
};
use reth_rpc::{
    eth::{
        cache::EthStateCache, gas_oracle::GasPriceOracle, EthFilterConfig, FeeHistoryCache,
        FeeHistoryCacheConfig,
    },
    secret_to_bearer_header, AuthClientLayer, AuthClientService, AuthLayer, EngineEthApi, EthApi,
    EthFilter, EthSubscriptionIdProvider, JwtAuthValidator, JwtSecret,
};
use reth_rpc_api::servers::*;
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner};
use reth_transaction_pool::TransactionPool;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tower::layer::util::Identity;

/// Configure and launch a _standalone_ auth server with `engine` and a _new_ `eth` namespace.
#[allow(clippy::too_many_arguments)]
pub async fn launch<Provider, Pool, Network, Tasks, EngineApi, EngineT, EvmConfig>(
    provider: Provider,
    pool: Pool,
    network: Network,
    executor: Tasks,
    engine_api: EngineApi,
    socket_addr: SocketAddr,
    secret: JwtSecret,
    evm_config: EvmConfig,
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
    EngineT: EngineTypes + 'static,
    EngineApi: EngineApiServer<EngineT>,
    EvmConfig: ConfigureEvm + 'static,
{
    // spawn a new cache task
    let eth_cache = EthStateCache::spawn_with(
        provider.clone(),
        Default::default(),
        executor.clone(),
        evm_config.clone(),
    );

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
        evm_config,
        None,
    );
    let config = EthFilterConfig::default()
        .max_logs_per_response(DEFAULT_MAX_LOGS_PER_RESPONSE)
        .max_blocks_per_filter(DEFAULT_MAX_BLOCKS_PER_FILTER);
    let eth_filter =
        EthFilter::new(provider, pool, eth_cache.clone(), config, Box::new(executor.clone()));
    launch_with_eth_api(eth_api, eth_filter, engine_api, socket_addr, secret).await
}

/// Configure and launch a _standalone_ auth server with existing EthApi implementation.
pub async fn launch_with_eth_api<Provider, Pool, Network, EngineApi, EngineT, EvmConfig>(
    eth_api: EthApi<Provider, Pool, Network, EvmConfig>,
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
    EngineT: EngineTypes + 'static,
    EngineApi: EngineApiServer<EngineT>,
    EvmConfig: ConfigureEvm + 'static,
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
        .set_http_middleware(middleware)
        .build(socket_addr)
        .await
        .map_err(|err| RpcError::server_error(err, ServerKind::Auth(socket_addr)))?;

    let local_addr = server
        .local_addr()
        .map_err(|err| RpcError::server_error(err, ServerKind::Auth(socket_addr)))?;

    let handle = server.start(module);

    Ok(AuthServerHandle { handle, local_addr, secret, ipc_endpoint: None, ipc_handle: None })
}

/// Server configuration for the auth server.
#[derive(Debug)]
pub struct AuthServerConfig {
    /// Where the server should listen.
    pub(crate) socket_addr: SocketAddr,
    /// The secret for the auth layer of the server.
    pub(crate) secret: JwtSecret,
    /// Configs for JSON-RPC Http.
    pub(crate) server_config: ServerBuilder<Identity, Identity>,
    /// Configs for IPC server
    pub(crate) ipc_server_config: Option<IpcServerBuilder<Identity, Identity>>,
    /// IPC endpoint
    pub(crate) ipc_endpoint: Option<String>,
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
        let Self { socket_addr, secret, server_config, ipc_server_config, ipc_endpoint } = self;

        // Create auth middleware.
        let middleware = tower::ServiceBuilder::new()
            .layer(AuthLayer::new(JwtAuthValidator::new(secret.clone())));

        // By default, both http and ws are enabled.
        let server = server_config
            .set_http_middleware(middleware)
            .build(socket_addr)
            .await
            .map_err(|err| RpcError::server_error(err, ServerKind::Auth(socket_addr)))?;

        let local_addr = server
            .local_addr()
            .map_err(|err| RpcError::server_error(err, ServerKind::Auth(socket_addr)))?;

        let handle = server.start(module.inner.clone());
        let mut ipc_handle: Option<reth_ipc::server::ServerHandle> = None;

        if let Some(ipc_server_config) = ipc_server_config {
            let ipc_endpoint_str = ipc_endpoint
                .clone()
                .unwrap_or_else(|| constants::DEFAULT_ENGINE_API_IPC_ENDPOINT.to_string());
            let ipc_server = ipc_server_config.build(ipc_endpoint_str);
            let res = ipc_server
                .start(module.inner)
                .await
                .map_err(reth_ipc::server::IpcServerStartError::from)?;
            ipc_handle = Some(res);
        }

        Ok(AuthServerHandle { handle, local_addr, secret, ipc_endpoint, ipc_handle })
    }
}

/// Builder type for configuring an `AuthServerConfig`.
#[derive(Debug)]
pub struct AuthServerConfigBuilder {
    socket_addr: Option<SocketAddr>,
    secret: JwtSecret,
    server_config: Option<ServerBuilder<Identity, Identity>>,
    ipc_server_config: Option<IpcServerBuilder<Identity, Identity>>,
    ipc_endpoint: Option<String>,
}

// === impl AuthServerConfigBuilder ===

impl AuthServerConfigBuilder {
    /// Create a new `AuthServerConfigBuilder` with the given `secret`.
    pub fn new(secret: JwtSecret) -> Self {
        Self {
            socket_addr: None,
            secret,
            server_config: None,
            ipc_server_config: None,
            ipc_endpoint: None,
        }
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
    pub fn with_server_config(mut self, config: ServerBuilder<Identity, Identity>) -> Self {
        self.server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Set the ipc endpoint for the server.
    pub fn ipc_endpoint(mut self, ipc_endpoint: String) -> Self {
        self.ipc_endpoint = Some(ipc_endpoint);
        self
    }

    /// Configures the IPC server
    ///
    /// Note: this always configures an [EthSubscriptionIdProvider]
    pub fn with_ipc_config(mut self, config: IpcServerBuilder<Identity, Identity>) -> Self {
        self.ipc_server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
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
                    .max_request_body_size(128 * 1024 * 1024)
                    .set_id_provider(EthSubscriptionIdProvider::default())
            }),
            ipc_server_config: self.ipc_server_config.map(|ipc_server_config| {
                ipc_server_config
                    .max_response_body_size(750 * 1024 * 1024)
                    .max_connections(500)
                    .max_request_body_size(128 * 1024 * 1024)
                    .set_id_provider(EthSubscriptionIdProvider::default())
            }),
            ipc_endpoint: self.ipc_endpoint,
        }
    }
}

/// Holds installed modules for the auth server.
#[derive(Debug, Clone)]
pub struct AuthRpcModule {
    pub(crate) inner: RpcModule<()>,
}

// === impl AuthRpcModule ===

impl AuthRpcModule {
    /// Create a new `AuthRpcModule` with the given `engine_api`.
    pub fn new<EngineApi, EngineT>(engine: EngineApi) -> Self
    where
        EngineT: EngineTypes + 'static,
        EngineApi: EngineApiServer<EngineT>,
    {
        let mut module = RpcModule::new(());
        module.merge(engine.into_rpc()).expect("No conflicting methods");
        Self { inner: module }
    }

    /// Get a reference to the inner `RpcModule`.
    pub fn module_mut(&mut self) -> &mut RpcModule<()> {
        &mut self.inner
    }

    /// Merge the given [Methods] in the configured authenticated methods.
    ///
    /// Fails if any of the methods in other is present already.
    pub fn merge_auth_methods(
        &mut self,
        other: impl Into<Methods>,
    ) -> Result<bool, RegisterMethodError> {
        self.module_mut().merge(other.into()).map(|_| true)
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
    handle: jsonrpsee::server::ServerHandle,
    secret: JwtSecret,
    ipc_endpoint: Option<String>,
    ipc_handle: Option<reth_ipc::server::ServerHandle>,
}

// === impl AuthServerHandle ===

impl AuthServerHandle {
    /// Returns the [`SocketAddr`] of the http server if started.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Tell the server to stop without waiting for the server to stop.
    pub fn stop(self) -> Result<(), AlreadyStoppedError> {
        self.handle.stop()
    }

    /// Returns the url to the http server
    pub fn http_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Returns the url to the ws server
    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.local_addr)
    }

    /// Returns a http client connected to the server.
    pub fn http_client(
        &self,
    ) -> jsonrpsee::http_client::HttpClient<AuthClientService<HttpBackend>> {
        // Create a middleware that adds a new JWT token to every request.
        let secret_layer = AuthClientLayer::new(self.secret.clone());
        let middleware = tower::ServiceBuilder::default().layer(secret_layer);
        jsonrpsee::http_client::HttpClientBuilder::default()
            .set_http_middleware(middleware)
            .build(self.http_url())
            .expect("Failed to create http client")
    }

    /// Returns a ws client connected to the server. Note that the connection can only be
    /// be established within 1 minute due to the JWT token expiration.
    pub async fn ws_client(&self) -> jsonrpsee::ws_client::WsClient {
        jsonrpsee::ws_client::WsClientBuilder::default()
            .set_headers(HeaderMap::from_iter([(
                AUTHORIZATION,
                secret_to_bearer_header(&self.secret),
            )]))
            .build(self.ws_url())
            .await
            .expect("Failed to create ws client")
    }

    /// Returns an ipc client connected to the server.
    #[cfg(unix)]
    pub async fn ipc_client(&self) -> Option<jsonrpsee::async_client::Client> {
        use reth_ipc::client::IpcClientBuilder;

        if let Some(ipc_endpoint) = self.ipc_endpoint.clone() {
            return Some(
                IpcClientBuilder::default()
                    .build(ipc_endpoint)
                    .await
                    .expect("Failed to create ipc client"),
            )
        }
        None
    }

    /// Returns an ipc handle
    pub fn ipc_handle(&self) -> Option<reth_ipc::server::ServerHandle> {
        self.ipc_handle.clone()
    }

    /// Return an ipc endpoint
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc_endpoint.clone()
    }
}
