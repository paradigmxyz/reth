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
    core::{
        middleware::layer::{Either as RpcEither, RpcLoggerLayer},
        JsonRawValue, TEN_MB_SIZE_BYTES,
    },
    server::{
        middleware::rpc::RpcServiceT, stop_channel, ConnectionGuard, ConnectionPermit, IdProvider,
        RandomIntegerIdProvider, ServerHandle, StopHandle,
    },
    BoundedSubscriptions, MethodResponse, MethodSink, Methods,
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
use tokio_util::task::AbortOnDropHandle;
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
    RpcMiddleware: Layer<RpcService, Service: RpcServiceT> + Clone + Send + 'static,
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
            Ok(listener) => {
                #[cfg(unix)]
                {
                    // set permissions only on unix
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(perms_str) = &self.cfg.ipc_socket_permissions &&
                        let Ok(mode) = u32::from_str_radix(&perms_str.replace("0o", ""), 8)
                    {
                        let perms = std::fs::Permissions::from_mode(mode);
                        let _ = std::fs::set_permissions(&self.endpoint, perms);
                    }
                }
                listener
            }
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
                        rpc_middleware: &self.rpc_middleware,
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

        // Drop the accept loop's sender so the channel closes once all connection tasks exit.
        drop(drop_on_completion);

        // Wait for all connection tasks to exit. Request tasks spawned by a connection are aborted
        // when it closes, but their cancellation is not awaited here.
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

impl<HttpMiddleware, RpcMiddleware> std::fmt::Debug for IpcServer<HttpMiddleware, RpcMiddleware> {
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
    pub const fn new() -> Self {
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
    ) -> RpcServiceBuilder<Stack<RpcEither<T, Identity>, L>> {
        let layer = if let Some(layer) = layer {
            RpcEither::Left(layer)
        } else {
            RpcEither::Right(Identity::new())
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
/// One instance serves a single IPC connection. The RPC middleware stack is built once per
/// connection and shared by all of its requests, so connection-scoped state such as the
/// subscription limit and per-connection middleware state spans the whole connection. Requests
/// still in flight when the connection closes are aborted, and the middleware is dropped once
/// their tasks have been cancelled.
///
/// # Note
/// This is similar to [`hyper::service::service_fn`](https://docs.rs/hyper/latest/hyper/service/fn.service_fn.html).
pub struct TowerServiceNoHttp<L: Layer<RpcService>> {
    /// The connection's RPC service wrapped in the RPC middleware.
    rpc_service: Arc<L::Service>,
    /// Connection permit.
    conn_permit: Arc<ConnectionPermit>,
    /// Maximum size in bytes of a request.
    max_request_body_size: usize,
    /// Maximum size in bytes of a response.
    max_response_body_size: usize,
}

impl<L: Layer<RpcService>> Clone for TowerServiceNoHttp<L> {
    fn clone(&self) -> Self {
        Self {
            rpc_service: self.rpc_service.clone(),
            conn_permit: self.conn_permit.clone(),
            max_request_body_size: self.max_request_body_size,
            max_response_body_size: self.max_response_body_size,
        }
    }
}

impl<L: Layer<RpcService>> std::fmt::Debug for TowerServiceNoHttp<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TowerServiceNoHttp")
            .field("conn_permit", &self.conn_permit)
            .field("max_request_body_size", &self.max_request_body_size)
            .field("max_response_body_size", &self.max_response_body_size)
            .finish_non_exhaustive()
    }
}

impl<RpcMiddleware> Service<String> for TowerServiceNoHttp<RpcMiddleware>
where
    RpcMiddleware: Layer<RpcService>,
    <RpcMiddleware as Layer<RpcService>>::Service:
        Send + Sync + 'static + RpcServiceT<MethodResponse = MethodResponse>,
{
    /// The serialized response, if the connection driver must write one.
    ///
    /// This is `None` for notifications and for subscription responses, which are sent through
    /// the connection's `MethodSink` instead. A subscription rejected by the subscription limit
    /// is an ordinary error response and yields `Some`.
    type Response = Option<String>;

    type Error = Box<dyn core::error::Error + Send + Sync + 'static>;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    /// Always ready; this service does not apply request backpressure.
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: String) -> Self::Future {
        trace!("{:?}", request);

        let max_response_body_size = self.max_response_body_size;
        let max_request_body_size = self.max_request_body_size;
        let conn = self.conn_permit.clone();
        let rpc_service = self.rpc_service.clone();
        // Run request parsing, middleware and handler execution, and response serialization in a
        // separate task so this work does not add latency to the connection's read/write loop.
        // Each task shares the connection's RPC service.
        //
        // The connection drops its pending calls when it closes, so the call must not outlive the
        // returned future, otherwise it keeps running without anyone waiting for the response.
        let f = AbortOnDropHandle::new(tokio::task::spawn(async move {
            ipc::call_with_service(
                request,
                &*rpc_service,
                max_response_body_size,
                max_request_body_size,
                conn,
            )
            .await
        }));

        Box::pin(async move {
            // Call panics are answered by the call itself. Anything left here has no request id to
            // respond to, and the connection writes errors verbatim, which would corrupt the
            // stream.
            Ok(f.await.unwrap_or_else(|err| {
                warn!(%err, "IPC call task failed");
                None
            }))
        })
    }
}

struct ProcessConnection<'a, HttpMiddleware, RpcMiddleware> {
    http_middleware: &'a tower::ServiceBuilder<HttpMiddleware>,
    rpc_middleware: &'a RpcServiceBuilder<RpcMiddleware>,
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
#[instrument(name = "connection", skip_all, fields(conn_id = %params.conn_id))]
fn process_connection<RpcMiddleware, HttpMiddleware>(
    params: ProcessConnection<'_, HttpMiddleware, RpcMiddleware>,
) where
    RpcMiddleware: Layer<RpcService> + Clone + Send + 'static,
    for<'a> <RpcMiddleware as Layer<RpcService>>::Service: RpcServiceT,
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

    let (tx, rx) = mpsc::channel::<Box<JsonRawValue>>(server_cfg.message_buffer_capacity as usize);
    let method_sink = MethodSink::new_with_limit(tx, server_cfg.max_response_body_size);
    let max_response_body_size = server_cfg.max_response_body_size as usize;

    // The middleware is built once per connection so that the subscription limit and any
    // per-connection middleware state are shared by all requests on this connection.
    let rpc_service = rpc_middleware.service(RpcService::new(
        methods,
        max_response_body_size,
        conn_id.into(),
        RpcServiceCfg {
            bounded_subscriptions: BoundedSubscriptions::new(
                server_cfg.max_subscriptions_per_connection,
            ),
            id_provider,
            sink: method_sink,
        },
    ));
    let tower_service = TowerServiceNoHttp::<RpcMiddleware> {
        rpc_service: Arc::new(rpc_service),
        conn_permit,
        max_request_body_size: server_cfg.max_request_body_size as usize,
        max_response_body_size,
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
    rx: mpsc::Receiver<Box<JsonRawValue>>,
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
                let Some(item) = item else { break };
                conn.push_back(String::from(Box::<str>::from(item)));
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
    /// The permissions to create the IPC socket with.
    ipc_socket_permissions: Option<String>,
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
            ipc_socket_permissions: None,
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

    /// Set the maximum number of subscriptions per connection. Default is 1024.
    pub const fn max_subscriptions_per_connection(mut self, max: u32) -> Self {
        self.settings.max_subscriptions_per_connection = max;
        self
    }

    /// The server enforces backpressure which means that
    /// `n` messages can be buffered and if the client
    /// can't keep up with the server.
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

    /// Sets the permissions for the IPC socket file.
    pub fn set_ipc_socket_permissions(mut self, permissions: Option<String>) -> Self {
        self.settings.ipc_socket_permissions = permissions;
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
    /// The middleware is built once per connection and shared by concurrent requests on that
    /// connection, so it must be `Send + Sync`. State mutated through `&self` requires thread-safe
    /// interior mutability, such as a mutex or atomics.
    ///
    /// The builder itself exposes a similar API as the [`tower::ServiceBuilder`]
    /// where it is possible to compose layers to the middleware.
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
#[expect(missing_docs)]
pub fn dummy_name() -> String {
    use rand::Rng;
    let num: u64 = rand::rng().random();
    format!(r"/tmp/my-uds-{num}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::IpcClientBuilder;
    use futures::future::select;
    use jsonrpsee::{
        core::{
            client::{self, ClientT, Error, Subscription, SubscriptionClientT},
            middleware::{Batch, BatchEntry, Notification},
            params::BatchRequestBuilder,
        },
        rpc_params,
        types::{error::TOO_MANY_SUBSCRIPTIONS_CODE, ErrorCode, Request},
        PendingSubscriptionSink, RpcModule, SubscriptionMessage,
    };
    use reth_tracing::init_test_tracing;
    use std::{
        pin::pin,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::broadcast;
    use tokio_stream::wrappers::BroadcastStream;

    #[tokio::test]
    #[cfg(unix)]
    async fn test_ipc_socket_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let endpoint = &dummy_name();
        let perms = "0777";
        let server = Builder::default()
            .set_ipc_socket_permissions(Some(perms.to_string()))
            .build(endpoint.clone());
        let module = RpcModule::new(());
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let meta = std::fs::metadata(endpoint).unwrap();
        let perms = meta.permissions();
        assert_eq!(perms.mode() & 0o777, 0o777);
    }

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
                    let raw_value = serde_json::value::to_raw_value(&item)?;
                    let notif = SubscriptionMessage::from(raw_value);

                    // NOTE: this will block until there a spot in the queue
                    // and you might want to do something smarter if it's
                    // critical that "the most recent item" must be sent when it is produced.
                    if sink.send(notif).await.is_err() {
                        break Ok(());
                    }

                    closed = c;
                }

                // Send back the error.
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_pending_call_aborted_on_disconnect() {
        init_test_tracing();

        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let (started_tx, started_rx) = oneshot::channel::<()>();
        let (dropped_tx, dropped_rx) = oneshot::channel::<()>();
        let mut module = RpcModule::new(std::sync::Mutex::new(Some((started_tx, dropped_tx))));
        module
            .register_async_method("hang", |_, ctx, _| async move {
                // `dropped_tx` is dropped together with the call
                let (started_tx, _dropped_tx) = ctx.lock().unwrap().take().unwrap();
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
                "unreachable"
            })
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        tokio::select! {
            _ = client.request::<String, _>("hang", rpc_params![]) => panic!("call completed"),
            _ = started_rx => {}
        }
        drop(client);

        let dropped = tokio::time::timeout(std::time::Duration::from_secs(5), dropped_rx).await;
        assert!(dropped.is_ok(), "call kept running after the connection closed");
    }

    #[tokio::test]
    async fn test_panicking_call_returns_internal_error() {
        init_test_tracing();

        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        module
            .register_async_method("maybe_panic", |params, _, _| async move {
                assert!(!params.one::<bool>().unwrap(), "requested panic");
                "ok"
            })
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let err = client.request::<String, _>("maybe_panic", rpc_params![true]).await.unwrap_err();
        assert!(
            matches!(&err, Error::Call(err) if err.code() == ErrorCode::InternalError.code()),
            "{err:?}"
        );

        let mut batch_request_builder = BatchRequestBuilder::new();
        let _ = batch_request_builder.insert("maybe_panic", rpc_params![true]);
        let _ = batch_request_builder.insert("maybe_panic", rpc_params![false]);
        let responses = client
            .batch_request::<String>(batch_request_builder)
            .await
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>();
        assert!(matches!(&responses[0], Err(err) if err.code() == ErrorCode::InternalError.code()));
        assert_eq!(responses[1].as_deref(), Ok("ok"));
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_max_subscriptions_per_connection() {
        let endpoint = &dummy_name();
        let server = Builder::default().max_subscriptions_per_connection(1).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module
            .register_subscription(
                "subscribe_hello",
                "s_hello",
                "unsubscribe_hello",
                |_, pending, _, _| async move {
                    let Ok(sink) = pending.accept().await else { return };
                    sink.closed().await;
                },
            )
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let _sub: Subscription<usize> =
            client.subscribe("subscribe_hello", rpc_params![], "unsubscribe_hello").await.unwrap();
        let err = client
            .subscribe::<usize, _>("subscribe_hello", rpc_params![], "unsubscribe_hello")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, Error::Call(err) if err.code() == TOO_MANY_SUBSCRIPTIONS_CODE),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_rpc_middleware() {
        #[derive(Clone)]
        struct ModifyRequestIf<S>(S);

        impl<S> RpcServiceT for ModifyRequestIf<S>
        where
            S: Send + Sync + RpcServiceT,
        {
            type MethodResponse = S::MethodResponse;
            type NotificationResponse = S::NotificationResponse;
            type BatchResponse = S::BatchResponse;

            fn call<'a>(
                &self,
                mut req: Request<'a>,
            ) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
                // Re-direct all calls that isn't `say_hello` to `say_goodbye`
                if req.method == "say_hello" {
                    req.method = "say_goodbye".into();
                } else if req.method == "say_goodbye" {
                    req.method = "say_hello".into();
                }

                self.0.call(req)
            }

            fn batch<'a>(
                &self,
                mut batch: Batch<'a>,
            ) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
                for call in batch.iter_mut() {
                    match call {
                        Ok(BatchEntry::Call(req)) => {
                            if req.method == "say_hello" {
                                req.method = "say_goodbye".into();
                            } else if req.method == "say_goodbye" {
                                req.method = "say_hello".into();
                            }
                        }
                        Ok(BatchEntry::Notification(n)) => {
                            if n.method == "say_hello" {
                                n.method = "say_goodbye".into();
                            } else if n.method == "say_goodbye" {
                                n.method = "say_hello".into();
                            }
                        }
                        // Invalid request, we don't care about it.
                        Err(_err) => {}
                    }
                }

                self.0.batch(batch)
            }

            fn notification<'a>(
                &self,
                mut n: Notification<'a>,
            ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
                if n.method == "say_hello" {
                    n.method = "say_goodbye".into();
                } else if n.method == "say_goodbye" {
                    n.method = "say_hello".into();
                }
                self.0.notification(n)
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

    #[tokio::test]
    async fn rpc_middleware_is_built_once_per_connection() {
        /// Counts how many middleware services are built and dropped.
        #[derive(Clone, Default)]
        struct CountingLayer {
            built: Arc<AtomicUsize>,
            dropped: Arc<AtomicUsize>,
        }

        impl<S> Layer<S> for CountingLayer {
            type Service = Counted<S>;

            fn layer(&self, inner: S) -> Self::Service {
                self.built.fetch_add(1, Ordering::SeqCst);
                Counted { inner, dropped: self.dropped.clone() }
            }
        }

        struct Counted<S> {
            inner: S,
            dropped: Arc<AtomicUsize>,
        }

        impl<S> Drop for Counted<S> {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }

        impl<S> RpcServiceT for Counted<S>
        where
            S: Send + Sync + RpcServiceT,
        {
            type MethodResponse = S::MethodResponse;
            type NotificationResponse = S::NotificationResponse;
            type BatchResponse = S::BatchResponse;

            fn call<'a>(
                &self,
                req: Request<'a>,
            ) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
                self.inner.call(req)
            }

            fn batch<'a>(
                &self,
                batch: Batch<'a>,
            ) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
                self.inner.batch(batch)
            }

            fn notification<'a>(
                &self,
                n: Notification<'a>,
            ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
                self.inner.notification(n)
            }
        }

        let endpoint = &dummy_name();
        let layer = CountingLayer::default();
        let server = Builder::default()
            .set_rpc_middleware(RpcServiceBuilder::new().layer(layer.clone()))
            .build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _, _| "ok").unwrap();
        let handle = server.start(module).await.unwrap();

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        for _ in 0..3 {
            client.request::<String, _>("anything", rpc_params![]).await.unwrap();
        }
        assert_eq!(layer.built.load(Ordering::SeqCst), 1);
        assert_eq!(layer.dropped.load(Ordering::SeqCst), 0);

        let other_client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        other_client.request::<String, _>("anything", rpc_params![]).await.unwrap();
        assert_eq!(layer.built.load(Ordering::SeqCst), 2);

        // With no requests in flight, closing the connection drops its middleware.
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while layer.dropped.load(Ordering::SeqCst) != 1 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("middleware was not dropped after the connection closed");

        // With no requests in flight, stopping the server drops the remaining connection's
        // middleware.
        handle.stop().unwrap();
        handle.stopped().await;
        assert_eq!(layer.dropped.load(Ordering::SeqCst), 2);
        drop(other_client);
    }
}
