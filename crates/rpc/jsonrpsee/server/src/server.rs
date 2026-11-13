// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::{
    borrow::Cow,
    fmt,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, OnceLock},
    task::Poll,
    time::Duration,
};

use crate::{
    future::{ConnectionGuard, ConnectionPermit, ServerHandle, StopHandle},
    middleware::rpc::{catch_call_panic, RpcService},
    transport::{http, ws, ws::BackgroundTaskParams},
    utils::{
        accept_error_backoff, deserialize_notification, no_request_within, NotifyOnRequest,
        PermitBody, WriteTimeoutIo, DEFAULT_FIRST_REQUEST_TIMEOUT, DEFAULT_PING_INTERVAL,
        DEFAULT_WRITE_TIMEOUT,
    },
    HttpBody, HttpRequest, HttpResponse, LOG_TARGET,
};

use futures_util::{
    future::{Either, FutureExt},
    io::{BufReader, BufWriter},
};
use hyper::body::{Bytes, Incoming};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    service::TowerToHyperService,
};
use jsonrpsee_core::{
    id_providers::RandomIntegerIdProvider,
    middleware::{Batch, BatchEntry, BatchEntryErr, RpcServiceBuilder, RpcServiceT},
    server::{
        helpers::prepare_error, BoundedSubscriptions, ConnectionId, MethodResponse, MethodSink,
        Methods,
    },
    traits::IdProvider,
    BoxError, JsonRawValue, TEN_MB_SIZE_BYTES,
};
use jsonrpsee_types::{
    error::{
        reject_too_big_batch_request, ErrorCode, BATCHES_NOT_SUPPORTED_CODE,
        BATCHES_NOT_SUPPORTED_MSG,
    },
    ErrorObject, Id, Notification, Request, TwoPointZero,
};
use serde::{
    de::{
        value::MapAccessDeserializer, DeserializeSeed, Deserializer, Error as _, IgnoredAny,
        MapAccess, SeqAccess, Visitor,
    },
    Deserialize,
};
use soketto::handshake::http::is_upgrade_request;
use tokio::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::{mpsc, watch, Notify},
};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tower::{layer::util::Identity, Layer, Service, ServiceExt};
use tracing::{instrument, Instrument};

/// Default maximum connections allowed.
const MAX_CONNECTIONS: u32 = 100;

/// JSON RPC server.
pub struct Server<HttpMiddleware = Identity, RpcMiddleware = Identity> {
    listener: TcpListener,
    server_cfg: ServerConfig,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
    http_middleware: tower::ServiceBuilder<HttpMiddleware>,
}

impl Server<Identity, Identity> {
    /// Create a builder for the server.
    pub fn builder() -> Builder<Identity, Identity> {
        Builder::new()
    }
}

impl<RpcMiddleware, HttpMiddleware> std::fmt::Debug for Server<RpcMiddleware, HttpMiddleware> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("listener", &self.listener)
            .field("server_cfg", &self.server_cfg)
            .finish()
    }
}

impl<RpcMiddleware, HttpMiddleware> Server<RpcMiddleware, HttpMiddleware> {
    /// Returns socket address to which the server is bound.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl<HttpMiddleware, RpcMiddleware, Body> Server<HttpMiddleware, RpcMiddleware>
where
    RpcMiddleware: Layer<RpcService> + Clone + Send + 'static,
    <RpcMiddleware as Layer<RpcService>>::Service: RpcServiceT,
    HttpMiddleware: Layer<TowerServiceNoHttp<RpcMiddleware>> + Send + 'static,
    <HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service:
        Service<HttpRequest, Response = HttpResponse<Body>, Error = BoxError> + Clone + Send,
    <<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service as Service<
        HttpRequest,
    >>::Future: Send,
    Body: http_body::Body<Data = Bytes> + Send + 'static,
    <Body as http_body::Body>::Error: Into<BoxError>,
{
    /// Start responding to connections requests.
    ///
    /// This will run on the tokio runtime until the server is stopped or the `ServerHandle` is
    /// dropped.
    pub fn start(mut self, methods: impl Into<Methods>) -> ServerHandle {
        let methods = methods.into();
        let (stop_tx, stop_rx) = watch::channel(());

        let stop_handle = StopHandle::new(stop_rx);

        match self.server_cfg.tokio_runtime.take() {
            Some(rt) => rt.spawn(self.start_inner(methods, stop_handle)),
            None => tokio::spawn(self.start_inner(methods, stop_handle)),
        };

        ServerHandle::new(stop_tx)
    }

    async fn start_inner(self, methods: Methods, stop_handle: StopHandle) {
        let connection_guard = ConnectionGuard::new(self.server_cfg.max_connections as usize);
        let listener = self.listener;

        let stopped = stop_handle.clone().shutdown();
        tokio::pin!(stopped);

        let (drop_on_completion, mut process_connection_awaiter) = mpsc::channel::<()>(1);

        loop {
            match try_accept_conn(&listener, stopped).await {
                AcceptConnection::Established { socket, remote_addr, stop } => {
                    process_connection(ProcessConnection {
                        http_middleware: &self.http_middleware,
                        rpc_middleware: self.rpc_middleware.clone(),
                        remote_addr,
                        methods: methods.clone(),
                        stop_handle: stop_handle.clone(),
                        conn_id: ConnectionId::next_unique().0,
                        server_cfg: self.server_cfg.clone(),
                        conn_guard: &connection_guard,
                        socket,
                        drop_on_completion: drop_on_completion.clone(),
                    });
                    stopped = stop;
                }
                AcceptConnection::Err((e, stop)) => {
                    tracing::debug!(target: LOG_TARGET, "Error while awaiting a new connection: {:?}", e);
                    let Some(stop) = accept_error_backoff(&e, stop).await else { break };
                    stopped = stop;
                }
                AcceptConnection::Shutdown => break,
            }
        }

        // Drop the last Sender
        drop(drop_on_completion);

        // Once this channel is closed it is safe to assume that all connections have been
        // gracefully shutdown
        while process_connection_awaiter.recv().await.is_some() {
            // Generally, messages should not be sent across this channel,
            // but we'll loop here to wait for `None` just to be on the safe side
        }
    }
}

/// Static server configuration which is shared per connection.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Maximum size in bytes of a request.
    pub(crate) max_request_body_size: u32,
    /// Maximum size in bytes of a response.
    pub(crate) max_response_body_size: u32,
    /// Maximum number of incoming connections allowed.
    pub(crate) max_connections: u32,
    /// Maximum number of subscriptions per connection.
    pub(crate) max_subscriptions_per_connection: u32,
    /// Whether batch requests are supported by this server or not.
    pub(crate) batch_requests_config: BatchRequestConfig,
    /// Custom tokio runtime to run the server on.
    pub(crate) tokio_runtime: Option<tokio::runtime::Handle>,
    /// Enable HTTP.
    pub(crate) enable_http: bool,
    /// Enable WS.
    pub(crate) enable_ws: bool,
    /// Number of messages that server is allowed to `buffer` until backpressure kicks in.
    pub(crate) message_buffer_capacity: u32,
    /// ID provider.
    pub(crate) id_provider: Arc<dyn IdProvider>,
    /// Timeout for receiving the first request of a connection.
    pub(crate) first_request_timeout: Option<Duration>,
    /// Timeout for writing a single message to a WebSocket connection and for a stalled HTTP
    /// response write.
    pub(crate) write_timeout: Duration,
    /// Interval at which WebSocket connections are pinged. A connection that completes neither a
    /// message nor a pong within twice this interval is closed.
    pub(crate) ping_interval: Duration,
}

/// The builder to configure and create a JSON-RPC server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfigBuilder(ServerConfig);

/// Configuration for batch request handling.
#[derive(Debug, Copy, Clone)]
pub enum BatchRequestConfig {
    /// Batch requests are disabled.
    Disabled,
    /// Each batch request is limited to `len` and any batch request bigger than `len` will not be
    /// processed.
    Limit(u32),
    /// The batch request is unlimited.
    Unlimited,
}

/// Connection related state that is needed
/// to execute JSON-RPC calls.
#[derive(Debug, Clone)]
pub struct ConnectionState {
    /// Stop handle.
    pub(crate) stop_handle: StopHandle,
    /// Connection ID
    pub(crate) conn_id: usize,
    /// Connection guard.
    pub(crate) _conn_permit: Arc<ConnectionPermit>,
}

impl ConnectionState {
    /// Create a new connection state.
    pub fn new(
        stop_handle: StopHandle,
        conn_id: usize,
        conn_permit: ConnectionPermit,
    ) -> ConnectionState {
        Self { stop_handle, conn_id, _conn_permit: Arc::new(conn_permit) }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig::builder().build()
    }
}

impl ServerConfig {
    /// Create a new builder for the [`ServerConfig`].
    pub fn builder() -> ServerConfigBuilder {
        ServerConfigBuilder::default()
    }
}

impl Default for ServerConfigBuilder {
    fn default() -> Self {
        Self(ServerConfig {
            max_request_body_size: TEN_MB_SIZE_BYTES,
            max_response_body_size: TEN_MB_SIZE_BYTES,
            max_connections: MAX_CONNECTIONS,
            max_subscriptions_per_connection: 1024,
            batch_requests_config: BatchRequestConfig::Unlimited,
            tokio_runtime: None,
            enable_http: true,
            enable_ws: true,
            message_buffer_capacity: 1024,
            id_provider: Arc::new(RandomIntegerIdProvider),
            first_request_timeout: Some(DEFAULT_FIRST_REQUEST_TIMEOUT),
            write_timeout: DEFAULT_WRITE_TIMEOUT,
            ping_interval: DEFAULT_PING_INTERVAL,
        })
    }
}

impl ServerConfigBuilder {
    /// Create a new [`ServerConfigBuilder`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum size of a request body in bytes. Default is 10 MiB.
    pub fn max_request_body_size(mut self, size: u32) -> Self {
        self.0.max_request_body_size = size;
        self
    }

    /// Set the maximum size of a response body in bytes. Default is 10 MiB.
    pub fn max_response_body_size(mut self, size: u32) -> Self {
        self.0.max_response_body_size = size;
        self
    }

    /// Set the maximum number of connections allowed. Default is 100.
    pub fn max_connections(mut self, max: u32) -> Self {
        self.0.max_connections = max;
        self
    }

    /// Set the maximum number of subscriptions per connection. Default is 1024.
    pub fn max_subscriptions_per_connection(mut self, max: u32) -> Self {
        self.0.max_subscriptions_per_connection = max;
        self
    }

    /// Configure how [batch requests](https://www.jsonrpc.org/specification#batch) shall be handled
    /// by the server.
    ///
    /// Default: batch requests are allowed and can be arbitrary big but the maximum payload size is
    /// limited.
    pub fn set_batch_request_config(mut self, cfg: BatchRequestConfig) -> Self {
        self.0.batch_requests_config = cfg;
        self
    }

    /// Configure a custom [`tokio::runtime::Handle`] to run the server on.
    ///
    /// Default: [`tokio::spawn`]
    pub fn custom_tokio_runtime(mut self, rt: tokio::runtime::Handle) -> Self {
        self.0.tokio_runtime = Some(rt);
        self
    }

    /// Configure the server to only serve JSON-RPC HTTP requests.
    ///
    /// Default: both http and ws are enabled.
    pub fn http_only(mut self) -> Self {
        self.0.enable_http = true;
        self.0.enable_ws = false;
        self
    }

    /// Configure the server to only serve JSON-RPC WebSocket requests.
    ///
    /// That implies that server just denies HTTP requests which isn't a WebSocket upgrade request
    ///
    /// Default: both http and ws are enabled.
    pub fn ws_only(mut self) -> Self {
        self.0.enable_http = false;
        self.0.enable_ws = true;
        self
    }

    /// The server enforces backpressure which means that
    /// `n` messages can be buffered and if the client
    /// can't keep with up the server.
    ///
    /// This `capacity` is applied per connection and
    /// applies globally on the connection which implies
    /// all JSON-RPC messages.
    ///
    /// It is the number of messages (responses and subscription notifications) that can wait to
    /// be written to the client, and also the most calls a WebSocket connection runs at once.
    ///
    /// For example if a subscription produces plenty of new items
    /// and the client can't keep up then no new messages are handled.
    ///
    /// If the client can't keep up then calls wait to queue their response. Once that many calls
    /// are running, the server stops reading new messages from the connection until one
    /// completes.
    ///
    /// # Panics
    ///
    /// Panics if the buffer capacity is 0.
    pub fn set_message_buffer_capacity(mut self, c: u32) -> Self {
        assert!(c > 0, "buffer capacity must be set to > 0");
        self.0.message_buffer_capacity = c;
        self
    }

    /// Configure custom `subscription ID` provider for the server to use
    /// to when getting new subscription calls.
    ///
    /// You may choose static dispatch or dynamic dispatch because
    /// `IdProvider` is implemented for `Box<T>`.
    ///
    /// Default: [`RandomIntegerIdProvider`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use jsonrpsee_server::{IdProvider, RandomStringIdProvider, ServerConfigBuilder};
    ///
    /// // static dispatch
    /// let builder1 = ServerConfigBuilder::default().set_id_provider(RandomStringIdProvider::new(16));
    ///
    /// // or dynamic dispatch
    /// let builder2 =
    ///     ServerConfigBuilder::default().set_id_provider(Box::new(RandomStringIdProvider::new(16)));
    /// ```
    pub fn set_id_provider<I: IdProvider + 'static>(mut self, id_provider: I) -> Self {
        self.0.id_provider = Arc::new(id_provider);
        self
    }

    /// Configure how long a connection may take to send its first request, e.g. the WebSocket
    /// upgrade request, before it is closed. `None` disables the timeout.
    ///
    /// Default is 30 seconds.
    pub fn set_first_request_timeout(mut self, first_request_timeout: Option<Duration>) -> Self {
        self.0.first_request_timeout = first_request_timeout;
        self
    }

    /// Configure how long writing a single message, ping or close frame to a WebSocket
    /// connection may take, and how long an HTTP response write may make no progress, before the
    /// connection is closed.
    ///
    /// A client that stops reading fills the socket's send buffer, which blocks the write. When
    /// the timeout expires, the connection is dropped together with its queued messages,
    /// subscriptions and pending calls, or with its buffered HTTP response.
    ///
    /// Default is 10 seconds.
    pub fn set_write_timeout(mut self, write_timeout: Duration) -> Self {
        self.0.write_timeout = write_timeout;
        self
    }

    /// Build the [`ServerConfig`].
    pub fn build(self) -> ServerConfig {
        self.0
    }
}

/// Builder to configure and create a JSON-RPC server.
#[derive(Debug)]
pub struct Builder<HttpMiddleware, RpcMiddleware> {
    server_cfg: ServerConfig,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
    http_middleware: tower::ServiceBuilder<HttpMiddleware>,
}

impl Default for Builder<Identity, Identity> {
    fn default() -> Self {
        Builder {
            server_cfg: ServerConfig::default(),
            rpc_middleware: RpcServiceBuilder::new(),
            http_middleware: tower::ServiceBuilder::new(),
        }
    }
}

impl Builder<Identity, Identity> {
    /// Create a default server builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a server builder with the given [`ServerConfig`].
    pub fn with_config(config: ServerConfig) -> Self {
        Self { server_cfg: config, ..Default::default() }
    }
}

impl<HttpMiddleware, RpcMiddleware> Builder<HttpMiddleware, RpcMiddleware> {
    /// Configure the [`ServerConfig`].
    pub fn set_config(mut self, cfg: ServerConfig) -> Self {
        self.server_cfg = cfg;
        self
    }

    /// Enable middleware that is invoked on every JSON-RPC call.
    ///
    /// The middleware itself is very similar to the `tower middleware` but
    /// it has a different service trait which takes &self instead &mut self
    /// which means that you can't use built-in middleware from tower.
    ///
    /// Another consequence of `&self` is that you must wrap any of the middleware state in
    /// a type which is Send and provides interior mutability such as `Arc<Mutex>`.
    ///
    /// The builder itself exposes a similar API as the [`tower::ServiceBuilder`]
    /// where it is possible to compose layers to the middleware.
    ///
    /// To add middleware [`RpcServiceBuilder`] exposes a few different layer APIs that
    /// are wrapped on top of the [`tower::ServiceBuilder`].
    ///
    /// When the server is started these layers are wrapped in the [`RpcService`] and
    /// that's why the service APIs is not exposed.
    /// ```
    /// use std::{
    ///     net::SocketAddr,
    ///     sync::{
    ///         atomic::{AtomicUsize, Ordering},
    ///         Arc,
    ///     },
    ///     time::Instant,
    /// };
    ///
    /// use jsonrpsee_server::{
    ///     middleware::rpc::{
    ///         Batch, MethodResponse, Notification, Request, RpcService, RpcServiceBuilder,
    ///         RpcServiceT,
    ///     },
    ///     ServerBuilder,
    /// };
    ///
    /// #[derive(Clone)]
    /// struct MyMiddleware<S> {
    ///     service: S,
    ///     count: Arc<AtomicUsize>,
    /// }
    ///
    /// impl<S> RpcServiceT for MyMiddleware<S>
    /// where
    ///     S: RpcServiceT + Clone + Send + Sync + 'static,
    /// {
    ///     fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
    ///         tracing::info!("MyMiddleware processed call {}", req.method);
    ///         let count = self.count.clone();
    ///         let service = self.service.clone();
    ///
    ///         async move {
    ///             let rp = service.call(req).await;
    ///             // Modify the state.
    ///             count.fetch_add(1, Ordering::Relaxed);
    ///             rp
    ///         }
    ///     }
    ///
    ///     fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
    ///         self.service.batch(batch)
    ///     }
    ///
    ///     fn notification<'a>(
    ///         &self,
    ///         notif: Notification<'a>,
    ///     ) -> impl Future<Output = MethodResponse> + Send + 'a {
    ///         self.service.notification(notif)
    ///     }
    /// }
    ///
    /// // Create a state per connection
    /// // NOTE: The service type can be omitted once `start` is called on the server.
    /// let m = RpcServiceBuilder::new().layer_fn(move |service: ()| MyMiddleware {
    ///     service,
    ///     count: Arc::new(AtomicUsize::new(0)),
    /// });
    /// let builder = ServerBuilder::default().set_rpc_middleware(m);
    /// ```
    pub fn set_rpc_middleware<T>(
        self,
        rpc_middleware: RpcServiceBuilder<T>,
    ) -> Builder<HttpMiddleware, T> {
        Builder {
            server_cfg: self.server_cfg,
            rpc_middleware,
            http_middleware: self.http_middleware,
        }
    }

    /// Configure a custom [`tower::ServiceBuilder`] middleware for composing layers to be applied
    /// to the RPC service.
    ///
    /// Default: No tower layers are applied to the RPC service.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::{net::SocketAddr, time::Duration};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let builder = tower::ServiceBuilder::new().timeout(Duration::from_secs(2));
    ///
    ///     let server = jsonrpsee_server::ServerBuilder::new()
    ///         .set_http_middleware(builder)
    ///         .build("127.0.0.1:0".parse::<SocketAddr>().unwrap())
    ///         .await
    ///         .unwrap();
    /// }
    /// ```
    pub fn set_http_middleware<T>(
        self,
        http_middleware: tower::ServiceBuilder<T>,
    ) -> Builder<T, RpcMiddleware> {
        Builder {
            server_cfg: self.server_cfg,
            http_middleware,
            rpc_middleware: self.rpc_middleware,
        }
    }

    /// Finalize the configuration of the server. Consumes the [`Builder`].
    ///
    /// ```rust
    /// #[tokio::main]
    /// async fn main() {
    ///     let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    ///     let occupied_addr = listener.local_addr().unwrap();
    ///     let addrs: &[std::net::SocketAddr] = &[occupied_addr, "127.0.0.1:0".parse().unwrap()];
    ///     assert!(jsonrpsee_server::ServerBuilder::default().build(occupied_addr).await.is_err());
    ///     assert!(jsonrpsee_server::ServerBuilder::default().build(addrs).await.is_ok());
    /// }
    /// ```
    pub async fn build(
        self,
        addrs: impl ToSocketAddrs,
    ) -> std::io::Result<Server<HttpMiddleware, RpcMiddleware>> {
        let listener = TcpListener::bind(addrs).await?;

        Ok(Server {
            listener,
            server_cfg: self.server_cfg,
            rpc_middleware: self.rpc_middleware,
            http_middleware: self.http_middleware,
        })
    }
}

/// Data required by the server to handle requests.
struct ServiceData<S> {
    /// Registered server methods.
    methods: Methods,
    /// Stop handle.
    stop_handle: StopHandle,
    /// Connection ID
    conn_id: usize,
    /// Connection guard.
    conn_guard: ConnectionGuard,
    /// ServerConfig
    server_cfg: ServerConfig,
    /// RPC service for the HTTP requests of the connection, built on the first one.
    http_rpc_service: OnceLock<Arc<S>>,
}

/// jsonrpsee tower service without HTTP specific middleware.
///
/// hyper clones the service for every request, so the connection's state is shared behind an
/// [`Arc`].
///
/// # Note
/// This is similar to [`hyper::service::service_fn`].
pub struct TowerServiceNoHttp<L: Layer<RpcService>> {
    inner: Arc<ServiceData<L::Service>>,
    rpc_middleware: RpcServiceBuilder<L>,
}

impl<L: Layer<RpcService> + Clone> Clone for TowerServiceNoHttp<L> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), rpc_middleware: self.rpc_middleware.clone() }
    }
}

// Not derived, a derive would require the RPC service of the middleware to be `Debug` as well.
impl<L: Layer<RpcService> + fmt::Debug> fmt::Debug for TowerServiceNoHttp<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TowerServiceNoHttp")
            .field("conn_id", &self.inner.conn_id)
            .field("rpc_middleware", &self.rpc_middleware)
            .finish_non_exhaustive()
    }
}

impl<Body, RpcMiddleware> Service<HttpRequest<Body>> for TowerServiceNoHttp<RpcMiddleware>
where
    RpcMiddleware: Layer<RpcService>,
    <RpcMiddleware as Layer<RpcService>>::Service: RpcServiceT + Send + Sync + 'static,
    Body: http_body::Body<Data = Bytes> + Send + 'static,
    <Body as http_body::Body>::Error: Into<BoxError>,
{
    type Response = HttpResponse;

    // The following associated type is required by the `impl<B, U, M: JsonRpcMiddleware> Server<B,
    // L>` bounds. It satisfies the server's bounds when the `tower::ServiceBuilder<B>` is not
    // set (ie `B: Identity`).
    type Error = BoxError;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: HttpRequest<Body>) -> Self::Future {
        let request = request.map(HttpBody::new);

        let conn_guard = &self.inner.conn_guard;
        let conn_id = self.inner.conn_id;

        tracing::trace!(target: LOG_TARGET, "{:?}", request);

        let Some(conn_permit) = conn_guard.try_acquire() else {
            return async move { Ok(http::response::too_many_requests()) }.boxed();
        };

        let max_conns = conn_guard.max_connections();
        let curr_conns = max_conns - conn_guard.available_connections();
        tracing::debug!(target: LOG_TARGET, "Accepting new connection {}/{}", curr_conns, max_conns);

        let is_upgrade_request = is_upgrade_request(&request);

        if self.inner.server_cfg.enable_ws && is_upgrade_request {
            let this = &self.inner;
            let conn = ConnectionState::new(this.stop_handle.clone(), conn_id, conn_permit);
            let server_cfg = this.server_cfg.clone();

            let mut server = soketto::handshake::http::Server::new();

            let response = match server.receive_request(&request) {
                Ok(response) => {
                    let (tx, rx) = mpsc::channel(this.server_cfg.message_buffer_capacity as usize);
                    let sink =
                        MethodSink::new_with_limit(tx, this.server_cfg.max_response_body_size);

                    // On each method call the `pending_calls` is cloned
                    // then when all pending_calls are dropped
                    // a graceful shutdown can occur.
                    let (pending_calls, pending_calls_completed) = mpsc::channel::<()>(1);

                    let rpc_service = RpcService::new(
                        this.methods.clone(),
                        this.server_cfg.max_response_body_size as usize,
                        this.conn_id.into(),
                        BoundedSubscriptions::new(this.server_cfg.max_subscriptions_per_connection),
                        sink.clone(),
                        this.server_cfg.id_provider.clone(),
                    );

                    let rpc_service = self.rpc_middleware.service(rpc_service);

                    tokio::spawn(
                        async move {
                            let upgraded = match hyper::upgrade::on(request).await {
                                Ok(u) => u,
                                Err(e) => {
                                    tracing::debug!(target: LOG_TARGET, "Could not upgrade connection: {}", e);
                                    return;
                                }
                            };

                            let io = TokioIo::new(upgraded);

                            let stream = BufReader::new(BufWriter::new(io.compat()));
                            let mut ws_builder = server.into_builder(stream);
                            ws_builder.set_max_message_size(server_cfg.max_request_body_size as usize);
                            let (sender, receiver) = ws_builder.finish();

                            let params = BackgroundTaskParams {
                                server_cfg,
                                conn,
                                ws_sender: sender,
                                ws_receiver: receiver,
                                rpc_service,
                                sink,
                                rx,
                                pending_calls,
                                pending_calls_completed,
                            };

                            ws::background_task(params).await;
                        }
                        .in_current_span(),
                    );

                    response.map(|()| HttpBody::empty())
                }
                Err(e) => {
                    tracing::debug!(target: LOG_TARGET, "Could not upgrade connection: {}", e);
                    HttpResponse::new(HttpBody::from(format!("Could not upgrade connection: {e}")))
                }
            };

            async { Ok(response) }.boxed()
        } else if self.inner.server_cfg.enable_http && !is_upgrade_request {
            let this = &self.inner;
            let max_response_size = this.server_cfg.max_response_body_size;
            let max_request_size = this.server_cfg.max_request_body_size;
            let batch_config = this.server_cfg.batch_requests_config;

            let rpc_service = this
                .http_rpc_service
                .get_or_init(|| {
                    Arc::new(self.rpc_middleware.service(RpcService::without_subscriptions(
                        this.methods.clone(),
                        max_response_size as usize,
                        this.conn_id.into(),
                    )))
                })
                .clone();

            Box::pin(async move {
                let rp =
                    http::call_with_service(request, batch_config, max_request_size, &*rpc_service)
                        .await;
                // NOTE: The `conn guard` must be held until the response body is written (or
                // dropped) to respect the `max_connections` limit, because hyper writes the body
                // after this future completes.
                Ok(rp.map(|body| HttpBody::new(PermitBody::new(body, conn_permit))))
            })
        } else {
            // NOTE: the `conn guard` is dropped when this function which is fine
            // because it doesn't rely on any async operations.
            Box::pin(async { Ok(http::response::denied()) })
        }
    }
}

struct ProcessConnection<'a, HttpMiddleware, RpcMiddleware> {
    http_middleware: &'a tower::ServiceBuilder<HttpMiddleware>,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
    conn_guard: &'a ConnectionGuard,
    conn_id: usize,
    server_cfg: ServerConfig,
    stop_handle: StopHandle,
    socket: TcpStream,
    drop_on_completion: mpsc::Sender<()>,
    remote_addr: SocketAddr,
    methods: Methods,
}

#[instrument(name = "connection", target = LOG_TARGET, skip_all, fields(remote_addr = %params.remote_addr, conn_id = %params.conn_id), level = "INFO")]
fn process_connection<RpcMiddleware, HttpMiddleware, Body>(
    params: ProcessConnection<HttpMiddleware, RpcMiddleware>,
) where
    RpcMiddleware: Layer<RpcService>,
    HttpMiddleware: Layer<TowerServiceNoHttp<RpcMiddleware>> + Send + 'static,
    <HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service: Service<HttpRequest, Response = HttpResponse<Body>, Error = BoxError>
        + Clone
        + Send
        + 'static,
    <<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service as Service<
        HttpRequest,
    >>::Future: Send + 'static,
    Body: http_body::Body<Data = Bytes> + Send + 'static,
    <Body as http_body::Body>::Error: Into<BoxError>,
{
    let ProcessConnection {
        http_middleware,
        rpc_middleware,
        conn_guard,
        conn_id,
        server_cfg,
        socket,
        stop_handle,
        drop_on_completion,
        methods,
        ..
    } = params;

    if let Err(e) = socket.set_nodelay(true) {
        tracing::warn!(target: LOG_TARGET, "Could not set NODELAY on socket: {:?}", e);
        return;
    }

    let first_request_timeout = server_cfg.first_request_timeout;
    let write_timeout = server_cfg.write_timeout;

    let tower_service = TowerServiceNoHttp {
        inner: Arc::new(ServiceData {
            server_cfg,
            methods,
            stop_handle: stop_handle.clone(),
            conn_id,
            conn_guard: conn_guard.clone(),
            http_rpc_service: OnceLock::new(),
        }),
        rpc_middleware,
    };

    let service = http_middleware.service(tower_service);

    let serve = async move {
        let request_received = Arc::new(Notify::new());
        // this requires Clone.
        let service = NotifyOnRequest::new(
            TowerToHyperService::new(
                service.map_request(|request: HttpRequest<Incoming>| request.map(HttpBody::new)),
            ),
            request_received.clone(),
        );
        let io = TokioIo::new(WriteTimeoutIo::new(socket, write_timeout));
        let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
        let conn = builder.serve_connection_with_upgrades(io, service);
        let stopped = stop_handle.shutdown();

        tokio::pin!(stopped, conn);

        let res = tokio::select! {
            res = &mut conn => res,
            _ = &mut stopped => {
                // NOTE: the connection should continue to be polled until shutdown can finish.
                // Thus, both lines below are needed and not a nit.
                conn.as_mut().graceful_shutdown();
                conn.await
            }
            err = no_request_within(&request_received, first_request_timeout) => Err(err),
        };

        if let Err(e) = res {
            tracing::debug!(target: LOG_TARGET, "HTTP serve connection failed {:?}", e);
        }
        drop(drop_on_completion)
    };
    // `#[instrument]` only spans this function, so the connection span is carried into the task.
    tokio::spawn(serve.in_current_span());
}

enum AcceptConnection<S> {
    Shutdown,
    Established { socket: TcpStream, remote_addr: SocketAddr, stop: S },
    Err((std::io::Error, S)),
}

async fn try_accept_conn<S>(listener: &TcpListener, stopped: S) -> AcceptConnection<S>
where
    S: Future + Unpin,
{
    let accept = listener.accept();
    tokio::pin!(accept);

    // `select` polls its first argument first and returns as soon as that one is ready, so the
    // stop signal has to come first: with `accept` first, a listener that keeps returning without
    // ever pending starves the stop signal and the server can no longer be stopped.
    match futures_util::future::select(stopped, accept).await {
        Either::Left(_) => AcceptConnection::Shutdown,
        Either::Right((res, stop)) => match res {
            Ok((socket, remote_addr)) => {
                AcceptConnection::Established { socket, remote_addr, stop }
            }
            Err(e) => AcceptConnection::Err((e, stop)),
        },
    }
}

/// Parses `body` as a single JSON-RPC message (`is_single`) or a batch, dispatches it to
/// `rpc_service` and returns the serialized response.
///
/// A panicking call is answered with an internal error. The caller sends the response only if it
/// [is a method call](MethodResponse::is_method_call) or [a batch](MethodResponse::is_batch):
/// notifications are not answered and subscription calls answer through the subscription sink.
pub async fn handle_rpc_call<S>(
    body: &[u8],
    is_single: bool,
    batch_config: BatchRequestConfig,
    rpc_service: &S,
) -> MethodResponse
where
    S: RpcServiceT + Send,
{
    // Single request or notification
    if is_single {
        if let Ok(req) = serde_json::from_slice::<Request<'_>>(body) {
            catch_call_panic(req.id(), || rpc_service.call(req)).await
        } else if let Ok(notif) = deserialize_notification(body) {
            rpc_service.notification(notif).await
        } else {
            let (id, code) = prepare_error(body);
            MethodResponse::error(id, ErrorObject::from(code))
        }
    }
    // Batch of requests.
    else {
        let max_len = match batch_config {
            BatchRequestConfig::Disabled => {
                let rp = MethodResponse::error(
                    Id::Null,
                    ErrorObject::borrowed(
                        BATCHES_NOT_SUPPORTED_CODE,
                        BATCHES_NOT_SUPPORTED_MSG,
                        None,
                    ),
                );
                return rp;
            }
            BatchRequestConfig::Limit(limit) => limit as usize,
            BatchRequestConfig::Unlimited => usize::MAX,
        };

        match parse_batch(body, max_len) {
            Ok(batch) => rpc_service.batch(batch).await,
            Err(rp) => rp,
        }
    }
}

/// Reads `body` as a batch of at most `max_len` entries, or returns the response to a batch that
/// can't be read.
fn parse_batch(body: &[u8], max_len: usize) -> Result<Batch<'_>, MethodResponse> {
    let mut entries = Vec::new();
    let mut too_big = false;

    // Most batches hold only valid requests, so the entries are decoded as they are read. Once an
    // entry doesn't decode, the batch is read again from that entry on, classifying each raw entry
    // to answer the invalid ones in place.
    for raw in [false, true] {
        let mut deserializer = serde_json::Deserializer::from_slice(body);
        let read = BatchSeed { max_len, too_big: &mut too_big, entries: &mut entries, raw }
            .deserialize(&mut deserializer)
            .and_then(|()| deserializer.end());

        if read.is_ok() {
            return Ok(Batch::from(entries));
        }
        // The seed stops reading as soon as the batch exceeds the limit, which surfaces as a
        // deserialization error.
        if too_big {
            return Err(MethodResponse::error(Id::Null, reject_too_big_batch_request(max_len)));
        }
    }

    Err(MethodResponse::error(Id::Null, ErrorObject::from(ErrorCode::ParseError)))
}

/// Classifies a raw batch entry as a call, a notification or an invalid request.
fn parse_batch_entry(call: &JsonRawValue) -> Result<BatchEntry<'_>, BatchEntryErr<'_>> {
    // The derived `Deserialize` impls below also accept a JSON array holding the fields in
    // order, so only objects may reach them.
    if !call.get().starts_with('{') {
        return Err(BatchEntryErr::new(Id::Null, ErrorCode::InvalidRequest.into()));
    }

    if let Ok(req) = serde_json::from_str(call.get()) {
        Ok(BatchEntry::Call(req))
    } else if let Ok(notif) = deserialize_notification(call.get().as_bytes()) {
        Ok(BatchEntry::Notification(notif))
    } else {
        let id = match serde_json::from_str::<jsonrpsee_types::InvalidRequest>(call.get()) {
            Ok(err) => err.id,
            Err(_) => Id::Null,
        };

        Err(BatchEntryErr::new(id, ErrorCode::InvalidRequest.into()))
    }
}

/// A valid batch entry: a call if it has an `id`, a notification otherwise.
///
/// Unknown members use the raw pass, which validates UTF-8 even in ignored values.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidBatchEntry<'a> {
    jsonrpc: TwoPointZero,
    #[serde(borrow, default, deserialize_with = "deserialize_present_id")]
    id: Option<Id<'a>>,
    #[serde(borrow)]
    method: Cow<'a, str>,
    #[serde(borrow)]
    params: Option<&'a JsonRawValue>,
}

impl<'a> From<ValidBatchEntry<'a>> for BatchEntry<'a> {
    fn from(entry: ValidBatchEntry<'a>) -> Self {
        let ValidBatchEntry { jsonrpc, id, method, params } = entry;
        let params = params.map(Cow::Borrowed);
        match id {
            Some(id) => Self::Call(Request { jsonrpc, id, method, params }),
            None => Self::Notification(Notification::new(method, params)),
        }
    }
}

/// Only runs for an `id` that is present, so `null` is an id and not a missing one.
fn deserialize_present_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Id<'de>>, D::Error> {
    Id::deserialize(deserializer).map(Some)
}

/// Decodes a [`ValidBatchEntry`] from a JSON object only, because its derived `Deserialize` impl
/// also accepts a JSON array holding the fields in order.
struct ValidBatchEntrySeed;

impl<'de> DeserializeSeed<'de> for ValidBatchEntrySeed {
    type Value = ValidBatchEntry<'de>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ValidBatchEntrySeed {
    type Value = ValidBatchEntry<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON-RPC request object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        ValidBatchEntry::deserialize(MapAccessDeserializer::new(map))
    }
}

/// Reads a JSON array of batch entries into `entries`.
///
/// Entries are decoded as they are read, or, if `raw`, classified from their raw JSON, after
/// skipping the entries that `entries` already holds.
///
/// Stops with an error as soon as an entry beyond `max_len` is read and sets `too_big`, so an
/// oversized batch is never collected.
struct BatchSeed<'a, 'de> {
    max_len: usize,
    too_big: &'a mut bool,
    entries: &'a mut Vec<Result<BatchEntry<'de>, BatchEntryErr<'de>>>,
    raw: bool,
}

impl<'de> DeserializeSeed<'de> for BatchSeed<'_, 'de> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for BatchSeed<'_, 'de> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a batch of JSON-RPC requests")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        if self.raw {
            for _ in 0..self.entries.len() {
                seq.next_element::<IgnoredAny>()?;
            }
        }

        loop {
            if self.entries.len() == self.max_len {
                // Read the excess entry without decoding or allocating its fields.
                if seq.next_element::<&'de JsonRawValue>()?.is_some() {
                    *self.too_big = true;
                    return Err(A::Error::custom("batch too big"));
                }
                return Ok(());
            }

            let entry = if self.raw {
                seq.next_element::<&'de JsonRawValue>()?.map(parse_batch_entry)
            } else {
                seq.next_element_seed(ValidBatchEntrySeed)?.map(|entry| Ok(entry.into()))
            };
            let Some(entry) = entry else { return Ok(()) };

            // Not preallocated from the size hint, which the input controls.
            self.entries.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future;
    use test_case::test_case;

    const CALL: &str = r#"{"jsonrpc":"2.0","method":"m","params":[1],"id":1}"#;

    const NOTIFICATION: &str = r#"{"jsonrpc":"2.0","method":"m"}"#;

    /// Classifies every entry of `body` from its raw JSON.
    fn classify_raw_batch(body: &str, max_len: usize) -> Option<String> {
        let (mut entries, mut too_big) = (Vec::new(), false);
        let mut deserializer = serde_json::Deserializer::from_str(body);
        BatchSeed { max_len, too_big: &mut too_big, entries: &mut entries, raw: true }
            .deserialize(&mut deserializer)
            .and_then(|()| deserializer.end())
            .ok()
            .map(|()| format!("{entries:?}"))
    }

    #[tokio::test]
    async fn try_accept_conn_observes_stop_before_a_waiting_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let _client = TcpStream::connect(addr).await.unwrap();
        // Let the kernel put the connection in the backlog, so that `accept` would hand it over on
        // its first poll and only the polling order keeps it waiting.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let accepted = try_accept_conn(&listener, future::ready(())).await;
        assert!(matches!(accepted, AcceptConnection::Shutdown));
    }

    #[test_case(&[CALL, CALL], usize::MAX; "calls")]
    #[test_case(&[CALL, NOTIFICATION, r#"{"jsonrpc":"2.0","method":"m","id":null}"#], usize::MAX; "call, notification and null id")]
    #[test_case(&[r#"{"id":"a","params":null,"method":"\"m\"","x":1,"jsonrpc":"2.0"}"#], usize::MAX; "escaped method, null params and unknown member")]
    #[test_case(&[CALL, r#"{"jsonrpc":"2.0","id":2}"#, NOTIFICATION, CALL], usize::MAX; "missing method between valid entries")]
    #[test_case(&[r#"{"jsonrpc":"1.0","method":"m","id":3}"#, CALL], usize::MAX; "wrong version first")]
    #[test_case(&[CALL, r#"{"jsonrpc":"2.0","method":"m","id":{}}"#], usize::MAX; "object id")]
    #[test_case(&[CALL, r#"{"jsonrpc":"2.0","method":"m","id":1,"id":2}"#], usize::MAX; "duplicate id")]
    #[test_case(&[CALL, r#"["2.0",2,"m",[]]"#, "7", CALL], usize::MAX; "array and number entries")]
    #[test_case(&[CALL, r#"{"id":2}"#], 2; "invalid entry at the limit")]
    #[test_case(&[CALL, r#"{"id":2}"#, CALL], 2; "valid entry past the limit")]
    #[test_case(&[CALL, CALL, r#"{"id":2}"#], 2; "invalid entry past the limit")]
    #[test_case(&[CALL, r#"{"jsonrpc""#], usize::MAX; "not json")]
    fn batch_entries_decode_like_their_raw_json(entries: &[&str], max_len: usize) {
        let body = format!("[{}]", entries.join(","));
        let batch = parse_batch(body.as_bytes(), max_len)
            .ok()
            .map(|batch| format!("{:?}", batch.into_iter().collect::<Vec<_>>()));
        assert_eq!(batch, classify_raw_batch(&body, max_len));
    }

    #[test]
    fn batch_with_invalid_utf8_is_a_parse_error() {
        // Unknown members must still be validated as UTF-8 by the raw pass.
        let body = b"[{\"jsonrpc\":\"2.0\",\"method\":\"m\",\"id\":1,\"x\":\"\xff\"}]";
        let rp = parse_batch(body, usize::MAX).unwrap_err();
        assert_eq!(rp.as_error_code(), Some(ErrorCode::ParseError.code()));
    }

    #[test]
    fn valid_batch_entries_are_calls_or_notifications() {
        let body = br#"[{"jsonrpc":"2.0","method":"a","id":null},{"jsonrpc":"2.0","method":"b"}]"#;
        let batch = parse_batch(body, usize::MAX).unwrap().into_iter().collect::<Vec<_>>();
        assert!(matches!(
            batch.as_slice(),
            [Ok(BatchEntry::Call(call)), Ok(BatchEntry::Notification(notification))]
                if call.id == Id::Null && call.method == "a" && notification.method == "b"
        ));
    }

    #[test_case(&[CALL.as_bytes(), CALL.as_bytes(), b"\"\xff\""], 1, reject_too_big_batch_request(1); "invalid utf8 after excess call")]
    #[test_case(&[br#"{"id":2}"#, CALL.as_bytes(), b"\"\xff\""], 1, reject_too_big_batch_request(1); "invalid utf8 after fallback exceeds limit")]
    #[test_case(&[CALL.as_bytes(), b"\"\xff\""], 1, ErrorCode::ParseError.into(); "invalid utf8 in excess entry")]
    #[test_case(&[b"{\"jsonrpc\":\"2.0\",\"method\":\"m\",\"x\":\"\xff\"}", CALL.as_bytes()], 1, ErrorCode::ParseError.into(); "invalid utf8 before excess entry")]
    #[test_case(&[CALL.as_bytes(), br#"{"jsonrpc"#], 1, ErrorCode::ParseError.into(); "malformed excess entry")]
    #[test_case(&[CALL.as_bytes(), b"7", b"\"\xff\""], 1, reject_too_big_batch_request(1); "invalid utf8 after excess scalar")]
    #[test_case(&[CALL.as_bytes(), b"\"\xff\""], 0, reject_too_big_batch_request(0); "zero limit ignores tail")]
    fn batch_limit_preserves_parse_errors(
        entries: &[&[u8]],
        max_len: usize,
        expected: ErrorObject<'_>,
    ) {
        let body = [b"[".as_slice(), entries.join(b",".as_slice()).as_slice(), b"]"].concat();
        assert_eq!(
            parse_batch(&body, max_len).unwrap_err().into_json(),
            MethodResponse::error(Id::Null, expected).into_json()
        );
    }

    #[test_case(b"[]", 0; "empty batch")]
    #[test_case(b"[{\"jsonrpc\":\"2.0\",\"method\":\"m\"}]", 1; "notification")]
    fn batch_at_limit_is_accepted(body: &[u8], max_len: usize) {
        assert_eq!(parse_batch(body, max_len).unwrap().len(), max_len);
    }
}
