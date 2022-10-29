//! JSON-RPC IPC server implementation

use crate::server::{
    connection::{Incoming, IpcConn, JsonRpcStream},
    future::{ConnectionGuard, FutureDriver, StopHandle},
};
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use jsonrpsee::{
    core::{
        server::{resource_limiting::Resources, rpc_module::Methods},
        Error, TEN_MB_SIZE_BYTES,
    },
    server::{logger::Logger, IdProvider, RandomIntegerIdProvider, ServerHandle},
};
use parity_tokio_ipc::Endpoint;

use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{watch, OwnedSemaphorePermit},
};
use tower::layer::util::Identity;

mod connection;
mod future;
mod ipc;

/// Ipc Server implementation

// This is an adapted `jsonrpsee` Server, but for `Ipc` connections.
pub struct IpcServer<B = Identity, L = ()> {
    /// The endpoint we listen for incoming transactions
    endpoint: Endpoint,
    resources: Resources,
    logger: L,
    id_provider: Arc<dyn IdProvider>,
    cfg: Settings,
    service_builder: tower::ServiceBuilder<B>,
}

impl IpcServer {
    /// Start responding to connections requests.
    ///
    /// This will run on the tokio runtime until the server is stopped or the ServerHandle is
    /// dropped.
    pub fn start(mut self, methods: impl Into<Methods>) -> Result<ServerHandle, Error> {
        let methods = methods.into().initialize_resources(&self.resources)?;
        let (stop_tx, stop_rx) = watch::channel(());

        let stop_handle = StopHandle::new(stop_rx);

        match self.cfg.tokio_runtime.take() {
            Some(rt) => rt.spawn(self.start_inner(methods, stop_handle)),
            None => tokio::spawn(self.start_inner(methods, stop_handle)),
        };

        Ok(ServerHandle::new(stop_tx))
    }

    async fn start_inner(self, methods: Methods, stop_handle: StopHandle) -> io::Result<()> {
        let max_request_body_size = self.cfg.max_request_body_size;
        let max_response_body_size = self.cfg.max_response_body_size;
        let max_log_length = self.cfg.max_log_length;
        let resources = self.resources;
        let id_provider = self.id_provider;
        let max_subscriptions_per_connection = self.cfg.max_subscriptions_per_connection;
        let logger = self.logger;

        let mut id: u32 = 0;
        let connection_guard = ConnectionGuard::new(self.cfg.max_connections as usize);

        let mut connections = FutureDriver::default();
        let incoming = match self.endpoint.incoming() {
            Ok(connections) => Incoming::new(connections),
            Err(err) => return Err(err),
        };
        let mut incoming = Monitored::new(incoming, &stop_handle);

        loop {
            match connections.select_with(&mut incoming).await {
                Ok(ipc) => {
                    let conn = match connection_guard.try_acquire() {
                        Some(conn) => conn,
                        None => {
                            tracing::warn!("Too many connections. Please try again later.");
                            connections.add(ipc.reject_connection().boxed());
                            continue
                        }
                    };

                    let tower_service = TowerService {
                        inner: ServiceData {
                            methods: methods.clone(),
                            resources: resources.clone(),
                            max_request_body_size,
                            max_response_body_size,
                            max_log_length,
                            id_provider: id_provider.clone(),
                            stop_handle: stop_handle.clone(),
                            max_subscriptions_per_connection,
                            conn_id: id,
                            logger: logger.clone(),
                            conn: Arc::new(conn),
                        },
                    };

                    let service = self.service_builder.service(tower_service);
                    connections.add(Box::pin(spawn_connection(ipc, service, stop_handle.clone())));

                    id = id.wrapping_add(1);
                }
                Err(MonitoredError::Selector(err)) => {
                    tracing::error!("Error while awaiting a new connection: {:?}", err);
                }
                Err(MonitoredError::Shutdown) => break,
            }
        }

        connections.await;
        Ok(())
    }
}

impl std::fmt::Debug for IpcServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcServer")
            .field("endpoint", &self.endpoint.path())
            .field("cfg", &self.cfg)
            .field("id_provider", &self.id_provider)
            .field("resources", &self.resources)
            .finish()
    }
}

/// Data required by the server to handle requests.
#[derive(Debug, Clone)]
#[allow(unused)]
pub(crate) struct ServiceData<L: Logger> {
    /// Registered server methods.
    pub(crate) methods: Methods,
    /// Tracker for currently used resources on the server.
    pub(crate) resources: Resources,
    /// Max request body size.
    pub(crate) max_request_body_size: u32,
    /// Max request body size.
    pub(crate) max_response_body_size: u32,
    /// Max length for logging for request and response
    ///
    /// Logs bigger than this limit will be truncated.
    pub(crate) max_log_length: u32,
    /// Subscription ID provider.
    pub(crate) id_provider: Arc<dyn IdProvider>,
    /// Stop handle.
    pub(crate) stop_handle: StopHandle,
    /// Max subscriptions per connection.
    pub(crate) max_subscriptions_per_connection: u32,
    /// Connection ID
    pub(crate) conn_id: u32,
    /// Logger.
    pub(crate) logger: L,
    /// Handle to hold a `connection permit`.
    pub(crate) conn: Arc<OwnedSemaphorePermit>,
}

/// JsonRPSee service compatible with `tower`.
///
/// # Note
/// This is similar to [`hyper::service::service_fn`].
#[derive(Debug)]
pub struct TowerService<L: Logger> {
    inner: ServiceData<L>,
}

impl<L: Logger> hyper::service::Service<String> for TowerService<L> {
    type Response = String;

    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    /// Opens door for back pressure implementation.
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: String) -> Self::Future {
        tracing::trace!("{:?}", request);

        // handle the request
        let data = ipc::HandleRequest {
            methods: self.inner.methods.clone(),
            resources: self.inner.resources.clone(),
            max_request_body_size: self.inner.max_request_body_size,
            max_response_body_size: self.inner.max_response_body_size,
            max_log_length: self.inner.max_log_length,
            batch_requests_supported: true,
            logger: self.inner.logger.clone(),
            conn: self.inner.conn.clone(),
        };
        Box::pin(ipc::handle_request(request, data).map(Ok))
    }
}

/// Spawns the connection in a new task
async fn spawn_connection<S, T>(
    conn: IpcConn<JsonRpcStream<T>>,
    mut service: S,
    mut stop_handle: StopHandle,
) where
    S: hyper::service::Service<String, Response = String> + Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let task = tokio::task::spawn(async move {
        tokio::pin!(conn);

        loop {
            let request = tokio::select! {
                res = conn.next() => {
                    match res {
                        Some(Ok(request)) => {
                            request
                        },
                        Some(Err(e)) => {
                             tracing::warn!("Request failed: {:?}", e);
                             break
                        }
                        None => {
                            return
                        }
                    }
                }
                _ = stop_handle.shutdown() => {
                    break
                }
            };

            // handle the RPC request
            let resp = match service.call(request).await {
                Ok(resp) => resp,
                Err(err) => err.into().to_string(),
            };

            // send back
            if let Err(err) = conn.send(resp).await {
                tracing::warn!("Failed to send response: {:?}", err);
                break
            }
        }
    });

    task.await.ok();
}

/// This is a glorified select listening for new messages, while also checking the `stop_receiver`
/// signal.
struct Monitored<'a, F> {
    future: F,
    stop_monitor: &'a StopHandle,
}

impl<'a, F> Monitored<'a, F> {
    fn new(future: F, stop_monitor: &'a StopHandle) -> Self {
        Monitored { future, stop_monitor }
    }
}

enum MonitoredError<E> {
    Shutdown,
    Selector(E),
}

impl<'a, T, Item> Future for Monitored<'a, Incoming<T, Item>>
where
    T: Stream<Item = io::Result<Item>> + Unpin + 'static,
    Item: AsyncRead + AsyncWrite,
{
    type Output = Result<IpcConn<JsonRpcStream<Item>>, MonitoredError<io::Error>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if this.stop_monitor.shutdown_requested() {
            return Poll::Ready(Err(MonitoredError::Shutdown))
        }

        this.future.poll_accept(cx).map_err(MonitoredError::Selector)
    }
}

/// JSON-RPC IPC server settings.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Maximum size in bytes of a request.
    max_request_body_size: u32,
    /// Maximum size in bytes of a response.
    max_response_body_size: u32,
    /// Max length for logging for requests and responses
    ///
    /// Logs bigger than this limit will be truncated.
    max_log_length: u32,
    /// Maximum number of incoming connections allowed.
    max_connections: u32,
    /// Maximum number of subscriptions per connection.
    max_subscriptions_per_connection: u32,
    /// Custom tokio runtime to run the server on.
    tokio_runtime: Option<tokio::runtime::Handle>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_request_body_size: TEN_MB_SIZE_BYTES,
            max_response_body_size: TEN_MB_SIZE_BYTES,
            max_log_length: 4096,
            max_connections: 100,
            max_subscriptions_per_connection: 1024,
            tokio_runtime: None,
        }
    }
}

/// Builder to configure and create a JSON-RPC server
#[derive(Debug)]
pub struct Builder<B = Identity, L = ()> {
    settings: Settings,
    resources: Resources,
    logger: L,
    id_provider: Arc<dyn IdProvider>,
    service_builder: tower::ServiceBuilder<B>,
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            settings: Settings::default(),
            resources: Resources::default(),
            logger: (),
            id_provider: Arc::new(RandomIntegerIdProvider),
            service_builder: tower::ServiceBuilder::new(),
        }
    }
}

impl<B, L> Builder<B, L> {
    /// Set the maximum size of a request body in bytes. Default is 10 MiB.
    pub fn max_request_body_size(mut self, size: u32) -> Self {
        self.settings.max_request_body_size = size;
        self
    }

    /// Set the maximum size of a response body in bytes. Default is 10 MiB.
    pub fn max_response_body_size(mut self, size: u32) -> Self {
        self.settings.max_response_body_size = size;
        self
    }

    /// Set the maximum size of a log
    pub fn max_log_length(mut self, size: u32) -> Self {
        self.settings.max_log_length = size;
        self
    }

    /// Set the maximum number of connections allowed. Default is 100.
    pub fn max_connections(mut self, max: u32) -> Self {
        self.settings.max_connections = max;
        self
    }

    /// Set the maximum number of connections allowed. Default is 1024.
    pub fn max_subscriptions_per_connection(mut self, max: u32) -> Self {
        self.settings.max_subscriptions_per_connection = max;
        self
    }

    /// Register a new resource kind. Errors if `label` is already registered, or if the number of
    /// registered resources on this server instance would exceed 8.
    ///
    /// See the module documentation for
    /// [`resurce_limiting`](../jsonrpsee_utils/server/resource_limiting/index.html#
    /// resource-limiting) for details.
    pub fn register_resource(
        mut self,
        label: &'static str,
        capacity: u16,
        default: u16,
    ) -> Result<Self, Error> {
        self.resources.register(label, capacity, default)?;
        Ok(self)
    }

    /// Add a logger to the builder [`Logger`](../jsonrpsee_core/logger/trait.Logger.html).
    ///
    /// ```
    /// use std::{time::Instant, net::SocketAddr};
    /// use jsonrpsee::server::logger::{HttpRequest, Logger, MethodKind, TransportProtocol};
    /// use jsonrpsee::types::Params;
    ///
    /// use jsonrpsee_server::logger::{Logger, HttpRequest, MethodKind, Params, TransportProtocol};
    /// use jsonrpsee_server::ServerBuilder;
    /// use reth_ipc::server::{Builder, IpcServer};
    ///
    /// #[derive(Clone)]
    /// struct MyLogger;
    ///
    /// impl Logger for MyLogger {
    ///     type Instant = Instant;
    ///
    ///     fn on_connect(&self, remote_addr: SocketAddr, request: &HttpRequest, transport: TransportProtocol) {
    ///          println!("[MyLogger::on_call] remote_addr: {:?}, headers: {:?}, transport: {}", remote_addr, request, transport);
    ///     }
    ///
    ///     fn on_request(&self, transport: TransportProtocol) -> Self::Instant {
    ///          Instant::now()
    ///     }
    ///
    ///     fn on_call(&self, method_name: &str, params: Params, kind: MethodKind, transport: TransportProtocol) {
    ///          println!("[MyLogger::on_call] method: '{}' params: {:?}, kind: {:?}, transport: {}", method_name, params, kind, transport);
    ///     }
    ///
    ///     fn on_result(&self, method_name: &str, success: bool, started_at: Self::Instant, transport: TransportProtocol) {
    ///          println!("[MyLogger::on_result] '{}', worked? {}, time elapsed {:?}, transport: {}", method_name, success, started_at.elapsed(), transport);
    ///     }
    ///
    ///     fn on_response(&self, result: &str, started_at: Self::Instant, transport: TransportProtocol) {
    ///          println!("[MyLogger::on_response] result: {}, time elapsed {:?}, transport: {}", result, started_at.elapsed(), transport);
    ///     }
    ///
    ///     fn on_disconnect(&self, remote_addr: SocketAddr, transport: TransportProtocol) {
    ///          println!("[MyLogger::on_disconnect] remote_addr: {:?}, transport: {}", remote_addr, transport);
    ///     }
    /// }
    ///
    /// let builder = Builder::default().set_logger(MyLogger);
    /// ```
    pub fn set_logger<T: Logger>(self, logger: T) -> Builder<B, T> {
        Builder {
            settings: self.settings,
            resources: self.resources,
            logger,
            id_provider: self.id_provider,
            service_builder: self.service_builder,
        }
    }

    /// Configure a custom [`tokio::runtime::Handle`] to run the server on.
    ///
    /// Default: [`tokio::spawn`]
    pub fn custom_tokio_runtime(mut self, rt: tokio::runtime::Handle) -> Self {
        self.settings.tokio_runtime = Some(rt);
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
    /// use jsonrpsee::server::RandomStringIdProvider;
    /// use reth_ipc::server::Builder;
    ///
    /// // static dispatch
    /// let builder1 = Builder::default().set_id_provider(RandomStringIdProvider::new(16));
    ///
    /// // or dynamic dispatch
    /// let builder2 = Builder::default().set_id_provider(Box::new(RandomStringIdProvider::new(16)));
    /// ```
    pub fn set_id_provider<I: IdProvider + 'static>(mut self, id_provider: I) -> Self {
        self.id_provider = Arc::new(id_provider);
        self
    }

    /// Configure a custom [`tower::ServiceBuilder`] middleware for composing layers to be applied
    /// to the RPC service.
    ///
    /// Default: No tower layers are applied to the RPC service.
    ///
    /// # Examples
    ///
    /// ```rust
    /// 
    /// use std::time::Duration;
    /// use std::net::SocketAddr;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let builder = tower::ServiceBuilder::new().timeout(Duration::from_secs(2));
    ///
    ///     let server = reth_ipc::server::Builder::default()
    ///         .set_middleware(builder)
    ///         .build()
    ///         .await
    ///         .unwrap();
    /// }
    /// ```
    pub fn set_middleware<T>(self, service_builder: tower::ServiceBuilder<T>) -> Builder<T, L> {
        Builder {
            settings: self.settings,
            resources: self.resources,
            logger: self.logger,
            id_provider: self.id_provider,
            service_builder,
        }
    }

    /// Finalize the configuration of the server. Consumes the [`Builder`].
    pub async fn build(self) -> Result<IpcServer<B, L>, Error> {
        todo!()
    }
}
