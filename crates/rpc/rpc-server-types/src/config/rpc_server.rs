use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use jsonrpsee::server::RpcServiceBuilder;
use reth_ipc::server::Builder as IpcServerBuilder;
use reth_rpc_layer::JwtSecret;
use tower::layer::util::Identity;

use crate::{constants, error::RpcError, ServerBuilder};

/// A builder type for configuring and launching the servers that will handle RPC requests.
///
/// Supported server transports are:
///    - http
///    - ws
///    - ipc
///
/// Http and WS share the same settings: [`ServerBuilder`].
///
/// Once the [RpcModule] is built via [RpcModuleBuilder] the servers can be started, See also
/// [ServerBuilder::build] and [Server::start](jsonrpsee::server::Server::start).
#[derive(Default, Debug)]
pub struct RpcServerConfig {
    /// Configs for JSON-RPC Http.
    http_server_config: Option<ServerBuilder<Identity, Identity>>,
    /// Allowed CORS Domains for http
    http_cors_domains: Option<String>,
    /// Address where to bind the http server to
    http_addr: Option<SocketAddr>,
    /// Configs for WS server
    ws_server_config: Option<ServerBuilder<Identity, Identity>>,
    /// Allowed CORS Domains for ws.
    ws_cors_domains: Option<String>,
    /// Address where to bind the ws server to
    ws_addr: Option<SocketAddr>,
    /// Configs for JSON-RPC IPC server
    ipc_server_config: Option<IpcServerBuilder<Identity, Identity>>,
    /// The Endpoint where to launch the ipc server
    ipc_endpoint: Option<String>,
    /// JWT secret for authentication
    jwt_secret: Option<JwtSecret>,
}

/// === impl RpcServerConfig ===

impl RpcServerConfig {
    /// Creates a new config with only http set
    pub fn http(config: ServerBuilder<Identity, Identity>) -> Self {
        Self::default().with_http(config)
    }

    /// Creates a new config with only ws set
    pub fn ws(config: ServerBuilder<Identity, Identity>) -> Self {
        Self::default().with_ws(config)
    }

    /// Creates a new config with only ipc set
    pub fn ipc(config: IpcServerBuilder<Identity, Identity>) -> Self {
        Self::default().with_ipc(config)
    }

    /// Configures the http server
    ///
    /// Note: this always configures an [EthSubscriptionIdProvider] [IdProvider] for convenience.
    /// To set a custom [IdProvider], please use [Self::with_id_provider].
    pub fn with_http(mut self, config: ServerBuilder<Identity, Identity>) -> Self {
        self.http_server_config =
            Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Configure the cors domains for http _and_ ws
    pub fn with_cors(self, cors_domain: Option<String>) -> Self {
        self.with_http_cors(cors_domain.clone()).with_ws_cors(cors_domain)
    }

    /// Configure the cors domains for HTTP
    pub fn with_http_cors(mut self, cors_domain: Option<String>) -> Self {
        self.http_cors_domains = cors_domain;
        self
    }

    /// Configure the cors domains for WS
    pub fn with_ws_cors(mut self, cors_domain: Option<String>) -> Self {
        self.ws_cors_domains = cors_domain;
        self
    }

    /// Configures the ws server
    ///
    /// Note: this always configures an [EthSubscriptionIdProvider] [IdProvider] for convenience.
    /// To set a custom [IdProvider], please use [Self::with_id_provider].
    pub fn with_ws(mut self, config: ServerBuilder<Identity, Identity>) -> Self {
        self.ws_server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Configures the [SocketAddr] of the http server
    ///
    /// Default is [Ipv4Addr::LOCALHOST] and [DEFAULT_HTTP_RPC_PORT]
    pub const fn with_http_address(mut self, addr: SocketAddr) -> Self {
        self.http_addr = Some(addr);
        self
    }

    /// Configures the [SocketAddr] of the ws server
    ///
    /// Default is [Ipv4Addr::LOCALHOST] and [DEFAULT_WS_RPC_PORT]
    pub const fn with_ws_address(mut self, addr: SocketAddr) -> Self {
        self.ws_addr = Some(addr);
        self
    }

    /// Configures the ipc server
    ///
    /// Note: this always configures an [EthSubscriptionIdProvider] [IdProvider] for convenience.
    /// To set a custom [IdProvider], please use [Self::with_id_provider].
    pub fn with_ipc(mut self, config: IpcServerBuilder<Identity, Identity>) -> Self {
        self.ipc_server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Sets a custom [IdProvider] for all configured transports.
    ///
    /// By default all transports use [EthSubscriptionIdProvider]
    pub fn with_id_provider<I>(mut self, id_provider: I) -> Self
    where
        I: IdProvider + Clone + 'static,
    {
        if let Some(http) = self.http_server_config {
            self.http_server_config = Some(http.set_id_provider(id_provider.clone()));
        }
        if let Some(ws) = self.ws_server_config {
            self.ws_server_config = Some(ws.set_id_provider(id_provider.clone()));
        }
        if let Some(ipc) = self.ipc_server_config {
            self.ipc_server_config = Some(ipc.set_id_provider(id_provider));
        }

        self
    }

    /// Configures the endpoint of the ipc server
    ///
    /// Default is [DEFAULT_IPC_ENDPOINT]
    pub fn with_ipc_endpoint(mut self, path: impl Into<String>) -> Self {
        self.ipc_endpoint = Some(path.into());
        self
    }

    /// Configures the JWT secret for authentication.
    pub const fn with_jwt_secret(mut self, secret: Option<JwtSecret>) -> Self {
        self.jwt_secret = secret;
        self
    }

    /// Returns true if any server is configured.
    ///
    /// If no server is configured, no server will be be launched on [RpcServerConfig::start].
    pub const fn has_server(&self) -> bool {
        self.http_server_config.is_some() ||
            self.ws_server_config.is_some() ||
            self.ipc_server_config.is_some()
    }

    /// Returns the [SocketAddr] of the http server
    pub const fn http_address(&self) -> Option<SocketAddr> {
        self.http_addr
    }

    /// Returns the [SocketAddr] of the ws server
    pub const fn ws_address(&self) -> Option<SocketAddr> {
        self.ws_addr
    }

    /// Returns the endpoint of the ipc server
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc_endpoint.clone()
    }

    /// Convenience function to do [RpcServerConfig::build] and [RpcServer::start] in one step
    pub async fn start(self, modules: TransportRpcModules) -> Result<RpcServerHandle, RpcError> {
        self.build(&modules).await?.start(modules).await
    }

    /// Creates the [CorsLayer] if any
    fn maybe_cors_layer(cors: Option<String>) -> Result<Option<CorsLayer>, CorsDomainError> {
        cors.as_deref().map(cors::create_cors_layer).transpose()
    }

    /// Creates the [AuthLayer] if any
    fn maybe_jwt_layer(&self) -> Option<AuthLayer<JwtAuthValidator>> {
        self.jwt_secret.map(|secret| AuthLayer::new(JwtAuthValidator::new(secret)))
    }

    /// Builds the ws and http server(s).
    ///
    /// If both are on the same port, they are combined into one server.
    async fn build_ws_http(
        &mut self,
        modules: &TransportRpcModules,
    ) -> Result<WsHttpServer, RpcError> {
        let http_socket_addr = self.http_addr.unwrap_or(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            constants::DEFAULT_HTTP_RPC_PORT,
        )));

        let ws_socket_addr = self.ws_addr.unwrap_or(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            constants::DEFAULT_WS_RPC_PORT,
        )));

        // If both are configured on the same port, we combine them into one server.
        if self.http_addr == self.ws_addr &&
            self.http_server_config.is_some() &&
            self.ws_server_config.is_some()
        {
            let cors = match (self.ws_cors_domains.as_ref(), self.http_cors_domains.as_ref()) {
                (Some(ws_cors), Some(http_cors)) => {
                    if ws_cors.trim() != http_cors.trim() {
                        return Err(WsHttpSamePortError::ConflictingCorsDomains {
                            http_cors_domains: Some(http_cors.clone()),
                            ws_cors_domains: Some(ws_cors.clone()),
                        }
                        .into())
                    }
                    Some(ws_cors)
                }
                (a, b) => a.or(b),
            }
            .cloned();

            // we merge this into one server using the http setup
            self.ws_server_config.take();

            modules.config.ensure_ws_http_identical()?;

            let builder = self.http_server_config.take().expect("http_server_config is Some");
            let server = builder
                .set_http_middleware(
                    tower::ServiceBuilder::new()
                        .option_layer(Self::maybe_cors_layer(cors)?)
                        .option_layer(self.maybe_jwt_layer()),
                )
                .set_rpc_middleware(
                    RpcServiceBuilder::new().layer(
                        modules
                            .http
                            .as_ref()
                            .or(modules.ws.as_ref())
                            .map(RpcRequestMetrics::same_port)
                            .unwrap_or_default(),
                    ),
                )
                .build(http_socket_addr)
                .await
                .map_err(|err| RpcError::server_error(err, ServerKind::WsHttp(http_socket_addr)))?;
            let addr = server
                .local_addr()
                .map_err(|err| RpcError::server_error(err, ServerKind::WsHttp(http_socket_addr)))?;
            return Ok(WsHttpServer {
                http_local_addr: Some(addr),
                ws_local_addr: Some(addr),
                server: WsHttpServers::SamePort(server),
                jwt_secret: self.jwt_secret,
            })
        }

        let mut http_local_addr = None;
        let mut http_server = None;

        let mut ws_local_addr = None;
        let mut ws_server = None;
        if let Some(builder) = self.ws_server_config.take() {
            let server = builder
                .ws_only()
                .set_http_middleware(
                    tower::ServiceBuilder::new()
                        .option_layer(Self::maybe_cors_layer(self.ws_cors_domains.clone())?)
                        .option_layer(self.maybe_jwt_layer()),
                )
                .set_rpc_middleware(
                    RpcServiceBuilder::new()
                        .layer(modules.ws.as_ref().map(RpcRequestMetrics::ws).unwrap_or_default()),
                )
                .build(ws_socket_addr)
                .await
                .map_err(|err| RpcError::server_error(err, ServerKind::WS(ws_socket_addr)))?;
            let addr = server
                .local_addr()
                .map_err(|err| RpcError::server_error(err, ServerKind::WS(ws_socket_addr)))?;

            ws_local_addr = Some(addr);
            ws_server = Some(server);
        }

        if let Some(builder) = self.http_server_config.take() {
            let server = builder
                .http_only()
                .set_http_middleware(
                    tower::ServiceBuilder::new()
                        .option_layer(Self::maybe_cors_layer(self.http_cors_domains.clone())?)
                        .option_layer(self.maybe_jwt_layer()),
                )
                .set_rpc_middleware(
                    RpcServiceBuilder::new().layer(
                        modules.http.as_ref().map(RpcRequestMetrics::http).unwrap_or_default(),
                    ),
                )
                .build(http_socket_addr)
                .await
                .map_err(|err| RpcError::server_error(err, ServerKind::Http(http_socket_addr)))?;
            let local_addr = server
                .local_addr()
                .map_err(|err| RpcError::server_error(err, ServerKind::Http(http_socket_addr)))?;
            http_local_addr = Some(local_addr);
            http_server = Some(server);
        }

        Ok(WsHttpServer {
            http_local_addr,
            ws_local_addr,
            server: WsHttpServers::DifferentPort { http: http_server, ws: ws_server },
            jwt_secret: self.jwt_secret,
        })
    }

    /// Finalize the configuration of the server(s).
    ///
    /// This consumes the builder and returns a server.
    ///
    /// Note: The server is not started and does nothing unless polled, See also [RpcServer::start]
    pub async fn build(mut self, modules: &TransportRpcModules) -> Result<RpcServer, RpcError> {
        let mut server = RpcServer::empty();
        server.ws_http = self.build_ws_http(modules).await?;

        if let Some(builder) = self.ipc_server_config {
            let metrics = modules.ipc.as_ref().map(RpcRequestMetrics::ipc).unwrap_or_default();
            let ipc_path =
                self.ipc_endpoint.unwrap_or_else(|| constants::DEFAULT_IPC_ENDPOINT.into());
            let ipc = builder
                .set_rpc_middleware(IpcRpcServiceBuilder::new().layer(metrics))
                .build(ipc_path);
            server.ipc = Some(ipc);
        }

        Ok(server)
    }
}
