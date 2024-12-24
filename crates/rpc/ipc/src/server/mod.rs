//! JSON-RPC IPC server implementation

use crate::server::connection::{IpcConn, JsonRpcStream};
use futures::StreamExt;
use futures_util::future::Either;
use interprocess::local_socket::{
    tokio::prelude::{LocalSocketListener, LocalSocketStream},
    traits::tokio::{Listener, Stream},
    GenericFilePath, ListenerOptions, ToFsName,
};
use jsonrpsee::{
    core::TEN_MB_SIZE_BYTES,
    server::{
        middleware::rpc::{RpcLoggerLayer, RpcServiceT},
        stop_channel, ConnectionGuard, ConnectionPermit, IdProvider, RandomIntegerIdProvider,
        ServerHandle, StopHandle,
    },
    BoundedSubscriptions, MethodSink, Methods,
};
use std::{
    future::Future,
    io,
    pin::{pin, Pin},
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::oneshot,
};
use tower::{layer::util::Identity, Layer, Service};
use tracing::{debug, instrument, trace, warn, Instrument};
// re-export so can be used during builder setup
use crate::{
    server::{connection::IpcConnDriver, rpc_service::RpcServiceCfg},
    stream_codec::StreamCodec,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower::layer::{util::Stack, LayerFn};

mod connection;
mod ipc;
mod rpc_service;

pub use rpc_service::RpcService;

/// Ipc Server implementation
///
/// This is an adapted `jsonrpsee` Server, but for `Ipc` connections.
pub struct IpcServer<HttpMiddleware = Identity, RpcMiddleware = Identity> {
    /// The endpoint we listen for incoming transactions
    endpoint: String,
    id_provider: Arc<dyn IdProvider>,
    cfg: Settings,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
    http_middleware: tower::ServiceBuilder<HttpMiddleware>,
}

impl<HttpMiddleware, RpcMiddleware> IpcServer<HttpMiddleware, RpcMiddleware> {
    /// Returns the configured endpoint
    pub fn endpoint(&self) -> String {
        self.endpoint.clone()
    }
}

impl<HttpMiddleware, RpcMiddleware> IpcServer<HttpMiddleware, RpcMiddleware>
where
    RpcMiddleware: for<'a> Layer<RpcService, Service: RpcServiceT<'a>> + Clone + Send + 'static,
    HttpMiddleware: Layer<
            TowerServiceNoHttp<RpcMiddleware>,
            Service: Service<
                String,
                Response = Option<String>,
                Error = Box<dyn core::error::Error + Send + Sync + 'static>,
                Future: Send + Unpin,
            > + Send,
        > + Send
        + 'static,
{
    /// Start responding to connections requests.
    ///
    /// This will run on the tokio runtime until the server is stopped or the `ServerHandle` is
    /// dropped.
    ///
    /// ```
    /// use jsonrpsee::RpcModule;
    /// use reth_ipc::server::Builder;
    /// async fn run_server() -> Result<(), Box<dyn core::error::Error + Send + Sync>> {
    ///     let server = Builder::default().build("/tmp/my-uds".into());
    ///     let mut module = RpcModule::new(());
    ///     module.register_method("say_hello", |_, _, _| "lo")?;
    ///     let handle = server.start(module).await?;
    ///
    ///     // In this example we don't care about doing shutdown so let's it run forever.
    ///     // You may use the `ServerHandle` to shut it down or manage it yourself.
    ///     let server = tokio::spawn(handle.stopped());
    ///     server.await.unwrap();
    ///     Ok(())
    /// }
    /// ```
    pub async fn start(
        mut self,
        methods: impl Into<Methods>,
    ) -> Result<ServerHandle, IpcServerStartError> {
        let methods = methods.into();

        let (stop_handle, server_handle) = stop_channel();

        // use a signal channel to wait until we're ready to accept connections
        let (tx, rx) = oneshot::channel();

        match self.cfg.tokio_runtime.take() {
            Some(rt) => rt.spawn(self.start_inner(methods, stop_handle, tx)),
            None => tokio::spawn(self.start_inner(methods, stop_handle, tx)),
        };
        rx.await.expect("channel is open")?;

        Ok(server_handle)
    }

    async fn start_inner(
        self,
        methods: Methods,
        stop_handle: StopHandle,
        on_ready: oneshot::Sender<Result<(), IpcServerStartError>>,
    ) {
        trace!(endpoint = ?self.endpoint, "starting ipc server");

        if cfg!(unix) {
            // ensure the file does not exist
            if std::fs::remove_file(&self.endpoint).is_ok() {
                debug!(endpoint = ?self.endpoint, "removed existing IPC endpoint file");
            }
        }

        let listener = match self
            .endpoint
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .and_then(|name| ListenerOptions::new().name(name).create_tokio())
        {
            Ok(listener) => listener,
            Err(err) => {
                on_ready
                    .send(Err(IpcServerStartError { endpoint: self.endpoint.clone(), source: err }))
                    .ok();
                return;
            }
        };

        // signal that we're ready to accept connections
        on_ready.send(Ok(())).ok();

        let mut id: u32 = 0;
        let connection_guard = ConnectionGuard::new(self.cfg.max_connections as usize);

        let stopped = stop_handle.clone().shutdown();
        let mut stopped = pin!(stopped);

        let (drop_on_completion, mut process_connection_awaiter) = mpsc::channel::<()>(1);

        trace!("accepting ipc connections");
        loop {
            match try_accept_conn(&listener, stopped).await {
                AcceptConnection::Established { local_socket_stream, stop } => {
                    let Some(conn_permit) = connection_guard.try_acquire() else {
                        let (_reader, mut writer) = local_socket_stream.split();
                        let _ = writer
                            .write_all(b"Too many connections. Please try again later.")
                            .await;
                        stopped = stop;
                        continue;
                    };

                    let max_conns = connection_guard.max_connections();
                    let curr_conns = max_conns - connection_guard.available_connections();
                    trace!("Accepting new connection {}/{}", curr_conns, max_conns);

                    let conn_permit = Arc::new(conn_permit);

                    process_connection(ProcessConnection {
                        http_middleware: &self.http_middleware,
                        rpc_middleware: self.rpc_middleware.clone(),
                        conn_permit,
                        conn_id: id,
                        server_cfg: self.cfg.clone(),
                        stop_handle: stop_handle.clone(),
                        drop_on_completion: drop_on_completion.clone(),
                        methods: methods.clone(),
                        id_provider: self.id_provider.clone(),
                        local_socket_stream,
                    });

                    id = id.wrapping_add(1);
                    stopped = stop;
                }
                AcceptConnection::Shutdown => {
                    break;
                }
                AcceptConnection::Err((err, stop)) => {
                    tracing::error!(%err, "Failed accepting a new IPC connection");
                    stopped = stop;
                }
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

enum AcceptConnection<S> {
    Shutdown,
    Established { local_socket_stream: LocalSocketStream, stop: S },
    Err((io::Error, S)),
}

async fn try_accept_conn<S>(listener: &LocalSocketListener, stopped: S) -> AcceptConnection<S>
where
    S: Future + Unpin,
{
    match futures_util::future::select(pin!(listener.accept()), stopped).await {
        Either::Left((res, stop)) => match res {
            Ok(local_socket_stream) => AcceptConnection::Established { local_socket_stream, stop },
            Err(e) => AcceptConnection::Err((e, stop)),
        },
        Either::Right(_) => AcceptConnection::Shutdown,
    }
}

impl std::fmt::Debug for IpcServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcServer")
            .field("endpoint", &self.endpoint)
            .field("cfg", &self.cfg)
            .field("id_provider", &self.id_provider)
            .finish()
    }
}

/// Error thrown when server couldn't be started.
#[derive(Debug, thiserror::Error)]
#[error("failed to listen on ipc endpoint `{endpoint}`: {source}")]
pub struct IpcServerStartError {
    endpoint: String,
    #[source]
    source: io::Error,
}

/// Data required by the server to handle requests received via an IPC connection
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ServiceData {
    /// Registered server methods.
    pub(crate) methods: Methods,
    /// Subscription ID provider.
    pub(crate) id_provider: Arc<dyn IdProvider>,
    /// Stop handle.
    pub(crate) stop_handle: StopHandle,
    /// Connection ID
    pub(crate) conn_id: u32,
    /// Connection Permit.
    pub(crate) conn_permit: Arc<ConnectionPermit>,
    /// Limits the number of subscriptions for this connection
    pub(crate) bounded_subscriptions: BoundedSubscriptions,
    /// Sink that is used to send back responses to the connection.
    ///
    /// This is used for subscriptions.
    pub(crate) method_sink: MethodSink,
    /// `ServerConfig`
    pub(crate) server_cfg: Settings,
}

/// Similar to [`tower::ServiceBuilder`] but doesn't
/// support any tower middleware implementations.
#[derive(Debug, Clone)]
pub struct RpcServiceBuilder<L>(tower::ServiceBuilder<L>);

impl Default for RpcServiceBuilder<Identity> {
    fn default() -> Self {
        Self(tower::ServiceBuilder::new())
    }
}

impl RpcServiceBuilder<Identity> {
    /// Create a new [`RpcServiceBuilder`].
    pub fn new() -> Self {
        Self(tower::ServiceBuilder::new())
    }
}

impl<L> RpcServiceBuilder<L> {
    /// Optionally add a new layer `T` to the [`RpcServiceBuilder`].
    ///
    /// See the documentation for [`tower::ServiceBuilder::option_layer`] for more details.
    pub fn option_layer<T>(
        self,
        layer: Option<T>,
    ) -> RpcServiceBuilder<Stack<Either<T, Identity>, L>> {
        let layer = if let Some(layer) = layer {
            Either::Left(layer)
        } else {
            Either::Right(Identity::new())
        };
        self.layer(layer)
    }

    /// Add a new layer `T` to the [`RpcServiceBuilder`].
    ///
    /// See the documentation for [`tower::ServiceBuilder::layer`] for more details.
    pub fn layer<T>(self, layer: T) -> RpcServiceBuilder<Stack<T, L>> {
        RpcServiceBuilder(self.0.layer(layer))
    }

    /// Add a [`tower::Layer`] built from a function that accepts a service and returns another
    /// service.
    ///
    /// See the documentation for [`tower::ServiceBuilder::layer_fn`] for more details.
    pub fn layer_fn<F>(self, f: F) -> RpcServiceBuilder<Stack<LayerFn<F>, L>> {
        RpcServiceBuilder(self.0.layer_fn(f))
    }

    /// Add a logging layer to [`RpcServiceBuilder`]
    ///
    /// This logs each request and response for every call.
    pub fn rpc_logger(self, max_log_len: u32) -> RpcServiceBuilder<Stack<RpcLoggerLayer, L>> {
        RpcServiceBuilder(self.0.layer(RpcLoggerLayer::new(max_log_len)))
    }

    /// Wrap the service `S` with the middleware.
    pub(crate) fn service<S>(&self, service: S) -> L::Service
    where
        L: tower::Layer<S>,
    {
        self.0.service(service)
    }
}

/// `JsonRPSee` service compatible with `tower`.
///
/// # Note
/// This is similar to [`hyper::service::service_fn`](https://docs.rs/hyper/latest/hyper/service/fn.service_fn.html).
#[derive(Debug, Clone)]
pub struct TowerServiceNoHttp<L> {
    inner: ServiceData,
    rpc_middleware: RpcServiceBuilder<L>,
}

impl<RpcMiddleware> Service<String> for TowerServiceNoHttp<RpcMiddleware>
where
    RpcMiddleware: for<'a> Layer<RpcService>,
    for<'a> <RpcMiddleware as Layer<RpcService>>::Service: Send + Sync + 'static + RpcServiceT<'a>,
{
    /// The response of a handled RPC call
    ///
    /// This is an `Option` because subscriptions and call responses are handled differently.
    /// This will be `Some` for calls, and `None` for subscriptions, because the subscription
    /// response will be emitted via the `method_sink`.
    type Response = Option<String>;

    type Error = Box<dyn core::error::Error + Send + Sync + 'static>;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    /// Opens door for back pressure implementation.
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: String) -> Self::Future {
        trace!("{:?}", request);

        let cfg = RpcServiceCfg::CallsAndSubscriptions {
            bounded_subscriptions: BoundedSubscriptions::new(
                self.inner.server_cfg.max_subscriptions_per_connection,
            ),
            id_provider: self.inner.id_provider.clone(),
            sink: self.inner.method_sink.clone(),
        };

        let max_response_body_size = self.inner.server_cfg.max_response_body_size as usize;
        let max_request_body_size = self.inner.server_cfg.max_request_body_size as usize;
        let conn = self.inner.conn_permit.clone();
        let rpc_service = self.rpc_middleware.service(RpcService::new(
            self.inner.methods.clone(),
            max_response_body_size,
            self.inner.conn_id.into(),
            cfg,
        ));
        // an ipc connection needs to handle read+write concurrently
        // even if the underlying rpc handler spawns the actual work or is does a lot of async any
        // additional overhead performed by `handle_request` can result in I/O latencies, for
        // example tracing calls are relatively CPU expensive on serde::serialize alone, moving this
        // work to a separate task takes the pressure off the connection so all concurrent responses
        // are also serialized concurrently and the connection can focus on read+write
        let f = tokio::task::spawn(async move {
            ipc::call_with_service(
                request,
                rpc_service,
                max_response_body_size,
                max_request_body_size,
                conn,
            )
            .await
        });

        Box::pin(async move { f.await.map_err(|err| err.into()) })
    }
}

struct ProcessConnection<'a, HttpMiddleware, RpcMiddleware> {
    http_middleware: &'a tower::ServiceBuilder<HttpMiddleware>,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
    conn_permit: Arc<ConnectionPermit>,
    conn_id: u32,
    server_cfg: Settings,
    stop_handle: StopHandle,
    drop_on_completion: mpsc::Sender<()>,
    methods: Methods,
    id_provider: Arc<dyn IdProvider>,
    local_socket_stream: LocalSocketStream,
}

/// Spawns the IPC connection onto a new task
#[instrument(name = "connection", skip_all, fields(conn_id = %params.conn_id), level = "INFO")]
fn process_connection<'b, RpcMiddleware, HttpMiddleware>(
    params: ProcessConnection<'_, HttpMiddleware, RpcMiddleware>,
) where
    RpcMiddleware: Layer<RpcService> + Clone + Send + 'static,
    for<'a> <RpcMiddleware as Layer<RpcService>>::Service: RpcServiceT<'a>,
    HttpMiddleware: Layer<TowerServiceNoHttp<RpcMiddleware>> + Send + 'static,
    <HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service: Send
    + Service<
        String,
        Response = Option<String>,
        Error = Box<dyn core::error::Error + Send + Sync + 'static>,
    >,
    <<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service as Service<String>>::Future:
    Send + Unpin,
 {
    let ProcessConnection {
        http_middleware,
        rpc_middleware,
        conn_permit,
        conn_id,
        server_cfg,
        stop_handle,
        drop_on_completion,
        id_provider,
        methods,
        local_socket_stream,
    } = params;

    let ipc = IpcConn(tokio_util::codec::Decoder::framed(
        StreamCodec::stream_incoming(),
        local_socket_stream,
    ));

    let (tx, rx) = mpsc::channel::<String>(server_cfg.message_buffer_capacity as usize);
    let method_sink = MethodSink::new_with_limit(tx, server_cfg.max_response_body_size);
    let tower_service = TowerServiceNoHttp {
        inner: ServiceData {
            methods,
            id_provider,
            stop_handle: stop_handle.clone(),
            server_cfg: server_cfg.clone(),
            conn_id,
            conn_permit,
            bounded_subscriptions: BoundedSubscriptions::new(
                server_cfg.max_subscriptions_per_connection,
            ),
            method_sink,
        },
        rpc_middleware,
    };

    let service = http_middleware.service(tower_service);
    tokio::spawn(async {
        to_ipc_service(ipc, service, stop_handle, rx).in_current_span().await;
        drop(drop_on_completion)
    });
}

async fn to_ipc_service<S, T>(
    ipc: IpcConn<JsonRpcStream<T>>,
    service: S,
    stop_handle: StopHandle,
    rx: mpsc::Receiver<String>,
) where
    S: Service<String, Response = Option<String>> + Send + 'static,
    S::Error: Into<Box<dyn core::error::Error + Send + Sync>>,
    S::Future: Send + Unpin,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let rx_item = ReceiverStream::new(rx);
    let conn = IpcConnDriver {
        conn: ipc,
        service,
        pending_calls: Default::default(),
        items: Default::default(),
    };
    let stopped = stop_handle.shutdown();

    let mut conn = pin!(conn);
    let mut rx_item = pin!(rx_item);
    let mut stopped = pin!(stopped);

    loop {
        tokio::select! {
            _ = &mut conn => {
               break
            }
            item = rx_item.next() => {
                if let Some(item) = item {
                    conn.push_back(item);
                }
            }
            _ = &mut stopped => {
                // shutdown
                break
            }
        }
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
    /// Number of messages that server is allowed `buffer` until backpressure kicks in.
    message_buffer_capacity: u32,
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
            message_buffer_capacity: 1024,
            tokio_runtime: None,
        }
    }
}

/// Builder to configure and create a JSON-RPC server
#[derive(Debug)]
pub struct Builder<HttpMiddleware, RpcMiddleware> {
    settings: Settings,
    /// Subscription ID provider.
    id_provider: Arc<dyn IdProvider>,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
    http_middleware: tower::ServiceBuilder<HttpMiddleware>,
}

impl Default for Builder<Identity, Identity> {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            id_provider: Arc::new(RandomIntegerIdProvider),
            rpc_middleware: RpcServiceBuilder::new(),
            http_middleware: tower::ServiceBuilder::new(),
        }
    }
}

impl<HttpMiddleware, RpcMiddleware> Builder<HttpMiddleware, RpcMiddleware> {
    /// Set the maximum size of a request body in bytes. Default is 10 MiB.
    pub const fn max_request_body_size(mut self, size: u32) -> Self {
        self.settings.max_request_body_size = size;
        self
    }

    /// Set the maximum size of a response body in bytes. Default is 10 MiB.
    pub const fn max_response_body_size(mut self, size: u32) -> Self {
        self.settings.max_response_body_size = size;
        self
    }

    /// Set the maximum size of a log
    pub const fn max_log_length(mut self, size: u32) -> Self {
        self.settings.max_log_length = size;
        self
    }

    /// Set the maximum number of connections allowed. Default is 100.
    pub const fn max_connections(mut self, max: u32) -> Self {
        self.settings.max_connections = max;
        self
    }

    /// Set the maximum number of connections allowed. Default is 1024.
    pub const fn max_subscriptions_per_connection(mut self, max: u32) -> Self {
        self.settings.max_subscriptions_per_connection = max;
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
    /// For example if a subscription produces plenty of new items
    /// and the client can't keep up then no new messages are handled.
    ///
    /// If this limit is exceeded then the server will "back-off"
    /// and only accept new messages once the client reads pending messages.
    ///
    /// # Panics
    ///
    /// Panics if the buffer capacity is 0.
    pub const fn set_message_buffer_capacity(mut self, c: u32) -> Self {
        self.settings.message_buffer_capacity = c;
        self
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
    /// #[tokio::main]
    /// async fn main() {
    ///     let builder = tower::ServiceBuilder::new();
    ///     let server = reth_ipc::server::Builder::default()
    ///         .set_http_middleware(builder)
    ///         .build("/tmp/my-uds".into());
    /// }
    /// ```
    pub fn set_http_middleware<T>(
        self,
        service_builder: tower::ServiceBuilder<T>,
    ) -> Builder<T, RpcMiddleware> {
        Builder {
            settings: self.settings,
            id_provider: self.id_provider,
            http_middleware: service_builder,
            rpc_middleware: self.rpc_middleware,
        }
    }

    /// Enable middleware that is invoked on every JSON-RPC call.
    ///
    /// The middleware itself is very similar to the `tower middleware` but
    /// it has a different service trait which takes &self instead &mut self
    /// which means that you can't use built-in middleware from tower.
    ///
    /// Another consequence of `&self` is that you must wrap any of the middleware state in
    /// a type which is Send and provides interior mutability such `Arc<Mutex>`.
    ///
    /// The builder itself exposes a similar API as the [`tower::ServiceBuilder`]
    /// where it is possible to compose layers to the middleware.
    ///
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
    /// use futures_util::future::BoxFuture;
    /// use jsonrpsee::{
    ///     server::{middleware::rpc::RpcServiceT, ServerBuilder},
    ///     types::Request,
    ///     MethodResponse,
    /// };
    /// use reth_ipc::server::{Builder, RpcServiceBuilder};
    ///
    /// #[derive(Clone)]
    /// struct MyMiddleware<S> {
    ///     service: S,
    ///     count: Arc<AtomicUsize>,
    /// }
    ///
    /// impl<'a, S> RpcServiceT<'a> for MyMiddleware<S>
    /// where
    ///     S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
    /// {
    ///     type Future = BoxFuture<'a, MethodResponse>;
    ///
    ///     fn call(&self, req: Request<'a>) -> Self::Future {
    ///         tracing::info!("MyMiddleware processed call {}", req.method);
    ///         let count = self.count.clone();
    ///         let service = self.service.clone();
    ///
    ///         Box::pin(async move {
    ///             let rp = service.call(req).await;
    ///             // Modify the state.
    ///             count.fetch_add(1, Ordering::Relaxed);
    ///             rp
    ///         })
    ///     }
    /// }
    ///
    /// // Create a state per connection
    /// // NOTE: The service type can be omitted once `start` is called on the server.
    /// let m = RpcServiceBuilder::new().layer_fn(move |service: ()| MyMiddleware {
    ///     service,
    ///     count: Arc::new(AtomicUsize::new(0)),
    /// });
    /// let builder = Builder::default().set_rpc_middleware(m);
    /// ```
    pub fn set_rpc_middleware<T>(
        self,
        rpc_middleware: RpcServiceBuilder<T>,
    ) -> Builder<HttpMiddleware, T> {
        Builder {
            settings: self.settings,
            id_provider: self.id_provider,
            rpc_middleware,
            http_middleware: self.http_middleware,
        }
    }

    /// Finalize the configuration of the server. Consumes the [`Builder`].
    pub fn build(self, endpoint: String) -> IpcServer<HttpMiddleware, RpcMiddleware> {
        IpcServer {
            endpoint,
            cfg: self.settings,
            id_provider: self.id_provider,
            http_middleware: self.http_middleware,
            rpc_middleware: self.rpc_middleware,
        }
    }
}

#[cfg(test)]
#[allow(missing_docs)]
pub fn dummy_name() -> String {
    let num: u64 = rand::Rng::gen(&mut rand::thread_rng());
    if cfg!(windows) {
        format!(r"\\.\pipe\my-pipe-{}", num)
    } else {
        format!(r"/tmp/my-uds-{}", num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::IpcClientBuilder;
    use futures::future::select;
    use jsonrpsee::{
        core::{
            client,
            client::{ClientT, Error, Subscription, SubscriptionClientT},
            params::BatchRequestBuilder,
        },
        rpc_params,
        types::Request,
        PendingSubscriptionSink, RpcModule, SubscriptionMessage,
    };
    use reth_tracing::init_test_tracing;
    use std::pin::pin;
    use tokio::sync::broadcast;
    use tokio_stream::wrappers::BroadcastStream;

    async fn pipe_from_stream_with_bounded_buffer(
        pending: PendingSubscriptionSink,
        stream: BroadcastStream<usize>,
    ) -> Result<(), Box<dyn core::error::Error + Send + Sync>> {
        let sink = pending.accept().await.unwrap();
        let closed = sink.closed();

        let mut closed = pin!(closed);
        let mut stream = pin!(stream);

        loop {
            match select(closed, stream.next()).await {
                // subscription closed or stream is closed.
                Either::Left((_, _)) | Either::Right((None, _)) => break Ok(()),

                // received new item from the stream.
                Either::Right((Some(Ok(item)), c)) => {
                    let notif = SubscriptionMessage::from_json(&item)?;

                    // NOTE: this will block until there a spot in the queue
                    // and you might want to do something smarter if it's
                    // critical that "the most recent item" must be sent when it is produced.
                    if sink.send(notif).await.is_err() {
                        break Ok(());
                    }

                    closed = c;
                }

                // Send back back the error.
                Either::Right((Some(Err(e)), _)) => break Err(e.into()),
            }
        }
    }

    // Naive example that broadcasts the produced values to all active subscribers.
    fn produce_items(tx: broadcast::Sender<usize>) {
        for c in 1..=100 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            let _ = tx.send(c);
        }
    }

    #[tokio::test]
    async fn can_set_the_max_response_body_size() {
        // init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().max_response_body_size(100).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _, _| "a".repeat(101)).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: Result<String, Error> = client.request("anything", rpc_params![]).await;
        assert!(response.unwrap_err().to_string().contains("Exceeded max limit of"));
    }

    #[tokio::test]
    async fn can_set_the_max_request_body_size() {
        init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().max_request_body_size(100).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _, _| "succeed").unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: Result<String, Error> =
            client.request("anything", rpc_params!["a".repeat(101)]).await;
        assert!(response.is_err());
        let mut batch_request_builder = BatchRequestBuilder::new();
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        // the raw request string is:
        //  [{"jsonrpc":"2.0","id":0,"method":"anything"},{"jsonrpc":"2.0","id":1, \
        //    "method":"anything"},{"jsonrpc":"2.0","id":2,"method":"anything"}]"
        // which is 136 bytes, more than 100 bytes.
        let response: Result<client::BatchResponse<'_, String>, Error> =
            client.batch_request(batch_request_builder).await;
        assert!(response.is_err());
    }

    #[tokio::test]
    async fn can_set_max_connections() {
        init_test_tracing();

        let endpoint = &dummy_name();
        let server = Builder::default().max_connections(2).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _, _| "succeed").unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client1 = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let client2 = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let client3 = IpcClientBuilder::default().build(endpoint).await.unwrap();

        let response1: Result<String, Error> = client1.request("anything", rpc_params![]).await;
        let response2: Result<String, Error> = client2.request("anything", rpc_params![]).await;
        let response3: Result<String, Error> = client3.request("anything", rpc_params![]).await;

        assert!(response1.is_ok());
        assert!(response2.is_ok());
        // Third connection is rejected
        assert!(response3.is_err());

        // Decrement connection count
        drop(client2);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Can connect again
        let client4 = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response4: Result<String, Error> = client4.request("anything", rpc_params![]).await;
        assert!(response4.is_ok());
    }

    #[tokio::test]
    async fn test_rpc_request() {
        init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        let msg = r#"{"jsonrpc":"2.0","id":83,"result":"0x7a69"}"#;
        module.register_method("eth_chainId", move |_, _, _| msg).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: String = client.request("eth_chainId", rpc_params![]).await.unwrap();
        assert_eq!(response, msg);
    }

    #[tokio::test]
    async fn test_batch_request() {
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _, _| "ok").unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let mut batch_request_builder = BatchRequestBuilder::new();
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let result = client
            .batch_request(batch_request_builder)
            .await
            .unwrap()
            .into_ok()
            .unwrap()
            .collect::<Vec<String>>();
        assert_eq!(result, vec!["ok", "ok", "ok"]);
    }

    #[tokio::test]
    async fn test_ipc_modules() {
        reth_tracing::init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        let msg = r#"{"admin":"1.0","debug":"1.0","engine":"1.0","eth":"1.0","ethash":"1.0","miner":"1.0","net":"1.0","rpc":"1.0","txpool":"1.0","web3":"1.0"}"#;
        module.register_method("rpc_modules", move |_, _, _| msg).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: String = client.request("rpc_modules", rpc_params![]).await.unwrap();
        assert_eq!(response, msg);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_rpc_subscription() {
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let (tx, _rx) = broadcast::channel::<usize>(16);

        let mut module = RpcModule::new(tx.clone());
        std::thread::spawn(move || produce_items(tx));

        module
            .register_subscription(
                "subscribe_hello",
                "s_hello",
                "unsubscribe_hello",
                |_, pending, tx, _| async move {
                    let rx = tx.subscribe();
                    let stream = BroadcastStream::new(rx);
                    pipe_from_stream_with_bounded_buffer(pending, stream).await?;
                    Ok(())
                },
            )
            .unwrap();

        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let sub: Subscription<usize> =
            client.subscribe("subscribe_hello", rpc_params![], "unsubscribe_hello").await.unwrap();

        let items = sub.take(16).collect::<Vec<_>>().await;
        assert_eq!(items.len(), 16);
    }

    #[tokio::test]
    async fn test_rpc_middleware() {
        #[derive(Clone)]
        struct ModifyRequestIf<S>(S);

        impl<'a, S> RpcServiceT<'a> for ModifyRequestIf<S>
        where
            S: Send + Sync + RpcServiceT<'a>,
        {
            type Future = S::Future;

            fn call(&self, mut req: Request<'a>) -> Self::Future {
                // Re-direct all calls that isn't `say_hello` to `say_goodbye`
                if req.method == "say_hello" {
                    req.method = "say_goodbye".into();
                } else if req.method == "say_goodbye" {
                    req.method = "say_hello".into();
                }

                self.0.call(req)
            }
        }

        reth_tracing::init_test_tracing();
        let endpoint = &dummy_name();

        let rpc_middleware = RpcServiceBuilder::new().layer_fn(ModifyRequestIf);
        let server = Builder::default().set_rpc_middleware(rpc_middleware).build(endpoint.clone());

        let mut module = RpcModule::new(());
        let goodbye_msg = r#"{"jsonrpc":"2.0","id":1,"result":"goodbye"}"#;
        let hello_msg = r#"{"jsonrpc":"2.0","id":2,"result":"hello"}"#;
        module.register_method("say_hello", move |_, _, _| hello_msg).unwrap();
        module.register_method("say_goodbye", move |_, _, _| goodbye_msg).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let say_hello_response: String = client.request("say_hello", rpc_params![]).await.unwrap();
        let say_goodbye_response: String =
            client.request("say_goodbye", rpc_params![]).await.unwrap();

        assert_eq!(say_hello_response, goodbye_msg);
        assert_eq!(say_goodbye_response, hello_msg);
    }
}
