//! JSON-RPC IPC server implementation

use crate::{
    server::connection::{IpcConn, IpcConnDriver, JsonRpcStream},
    stream_codec::StreamCodec,
};
use alloy_rpc_server::{
    error::ErrorCode,
    handle_rpc_call,
    server::{stop_channel, ServerHandle, StopHandle, DEFAULT_SIZE_LIMIT},
    BatchRequestConfig, Connection, ConnectionId, Id, IdProvider, MethodResponse, Methods,
    RandomIntegerIdProvider, RpcServiceT,
};
use futures::StreamExt;
use futures_util::future::Either;
use interprocess::local_socket::{
    tokio::prelude::{LocalSocketListener, LocalSocketStream},
    traits::tokio::{Listener, Stream},
    GenericFilePath, ListenerOptions, ToFsName,
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
    sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore},
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::task::AbortOnDropHandle;
use tower::{layer::util::Identity, Layer, Service};
use tracing::{instrument, trace, warn, Instrument};

#[cfg(unix)]
use interprocess::os::unix::local_socket::ListenerOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub use alloy_rpc_server::{server::RpcServiceBuilder, RpcService};

mod connection;

/// Number of messages a connection buffers before backpressure kicks in.
const MESSAGE_BUFFER_CAPACITY: usize = 1024;

/// Ipc Server implementation
///
/// A JSON-RPC server for `Ipc` connections.
pub struct IpcServer<RpcMiddleware = Identity> {
    /// The endpoint we listen for incoming transactions
    endpoint: String,
    id_provider: Arc<dyn IdProvider>,
    cfg: Settings,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
}

impl<RpcMiddleware> IpcServer<RpcMiddleware> {
    /// Returns the configured endpoint
    pub fn endpoint(&self) -> String {
        self.endpoint.clone()
    }
}

impl<RpcMiddleware> IpcServer<RpcMiddleware>
where
    RpcMiddleware: Layer<RpcService> + Clone + Send + 'static,
    RpcMiddleware::Service: RpcServiceT + Send + Sync + 'static,
{
    /// Start responding to connections requests.
    ///
    /// This will run on the tokio runtime until the server is stopped or the `ServerHandle` is
    /// dropped.
    ///
    /// Returns an error if the endpoint cannot be opened or the requested Unix socket permissions
    /// cannot be parsed or applied before serving requests.
    ///
    /// ```
    /// use alloy_rpc_server::{ErrorPayload, RpcModule};
    /// use reth_ipc::server::Builder;
    /// async fn run_server() -> Result<(), Box<dyn core::error::Error + Send + Sync>> {
    ///     let server = Builder::default().build("/tmp/my-uds".into());
    ///     let mut module = RpcModule::new(());
    ///     module.register_method("say_hello", |(): (), _| Ok::<_, ErrorPayload>("lo"))?;
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

        let listener =
            match self.endpoint.as_str().to_fs_name::<GenericFilePath>().and_then(|name| {
                // Unlinks an existing endpoint only after binding to it fails, so earlier errors
                // such as an unsupported mode leave it in place.
                let options = ListenerOptions::new().name(name).try_overwrite(true);

                #[cfg(unix)]
                let mode = self
                    .cfg
                    .ipc_socket_permissions
                    .as_deref()
                    .map(|permissions| {
                        let mode = u32::from_str_radix(
                            permissions.strip_prefix("0o").unwrap_or(permissions),
                            8,
                        )
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
                        if mode > 0o7777 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                format!("IPC socket permissions `{permissions}` exceed 0o7777"),
                            ));
                        }
                        Ok(mode)
                    })
                    .transpose()?;

                #[cfg(unix)]
                let options = if let Some(mode) = mode {
                    // Set the mode before bind/listen so connections cannot bypass it during
                    // startup. The range check keeps the cast lossless where `mode_t` is `u16`.
                    options.mode(mode as _)
                } else {
                    options
                };

                let listener = options.create_tokio()?;

                #[cfg(unix)]
                if let Some(mode) = mode {
                    // Binding can further restrict the requested mode through the process umask.
                    std::fs::set_permissions(
                        &self.endpoint,
                        std::fs::Permissions::from_mode(mode),
                    )?;
                }

                Ok(listener)
            }) {
                Ok(listener) => listener,
                Err(err) => {
                    on_ready
                        .send(Err(IpcServerStartError {
                            endpoint: self.endpoint.clone(),
                            source: err,
                        }))
                        .ok();
                    return;
                }
            };

        // signal that we're ready to accept connections
        on_ready.send(Ok(())).ok();

        let mut id: u32 = 0;
        let max_connections = self.cfg.max_connections as usize;
        let connections = Arc::new(Semaphore::new(max_connections));

        let stopped = stop_handle.clone().shutdown();
        let mut stopped = pin!(stopped);

        let (drop_on_completion, mut process_connection_awaiter) = mpsc::channel::<()>(1);

        trace!("accepting ipc connections");
        loop {
            match try_accept_conn(&listener, stopped).await {
                AcceptConnection::Established { local_socket_stream, stop } => {
                    let Ok(conn_permit) = connections.clone().try_acquire_owned() else {
                        let (_reader, mut writer) = local_socket_stream.split();
                        let _ = writer
                            .write_all(b"Too many connections. Please try again later.")
                            .await;
                        stopped = stop;
                        continue;
                    };

                    let open = max_connections.saturating_sub(connections.available_permits());
                    trace!("Accepting new connection {open}/{max_connections}");

                    process_connection(ProcessConnection {
                        rpc_middleware: self.rpc_middleware.clone(),
                        conn_permit: Arc::new(conn_permit),
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

impl<RpcMiddleware> std::fmt::Debug for IpcServer<RpcMiddleware> {
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
struct ServiceData {
    /// Registered server methods.
    methods: Methods,
    /// Connection Permit.
    conn_permit: Arc<OwnedSemaphorePermit>,
    /// The connection, which carries its subscriptions and sends their notifications.
    connection: Connection,
    /// `ServerConfig`
    server_cfg: Settings,
}

/// Turns raw IPC messages into serialized JSON-RPC responses.
#[derive(Debug, Clone)]
struct TowerServiceNoHttp<L> {
    inner: ServiceData,
    rpc_middleware: RpcServiceBuilder<L>,
}

impl<RpcMiddleware> Service<String> for TowerServiceNoHttp<RpcMiddleware>
where
    RpcMiddleware: Layer<RpcService>,
    RpcMiddleware::Service: RpcServiceT + Send + Sync + 'static,
{
    /// The response of a handled RPC call
    ///
    /// This is an `Option` because subscriptions and call responses are handled differently.
    /// This will be `Some` for calls, and `None` for subscriptions and notifications, because the
    /// subscription response will be emitted via the `method_sink`.
    type Response = Option<String>;

    type Error = Box<dyn core::error::Error + Send + Sync + 'static>;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    /// Opens door for back pressure implementation.
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: String) -> Self::Future {
        trace!(request_bytes = request.len(), "Received IPC request");

        let max_request_body_size = self.inner.server_cfg.max_request_body_size;
        let conn = self.inner.conn_permit.clone();
        let rpc_service = self.rpc_middleware.service(RpcService::with_connection(
            self.inner.methods.clone(),
            self.inner.server_cfg.max_response_body_size as usize,
            self.inner.connection.clone(),
        ));
        // an ipc connection needs to handle read+write concurrently
        // even if the underlying rpc handler spawns the actual work or is does a lot of async any
        // additional overhead performed by `handle_request` can result in I/O latencies, for
        // example tracing calls are relatively CPU expensive on serde::serialize alone, moving this
        // work to a separate task takes the pressure off the connection so all concurrent responses
        // are also serialized concurrently and the connection can focus on read+write
        //
        // The connection drops its pending calls when it closes, so the call must not outlive the
        // returned future, otherwise it keeps running without anyone waiting for the response.
        let f = AbortOnDropHandle::new(tokio::task::spawn(async move {
            let rp = if request.len() > max_request_body_size as usize {
                MethodResponse::error(
                    Id::None,
                    ErrorCode::OversizedRequest.exceeded(max_request_body_size),
                )
            } else {
                let is_single = request.chars().find_map(|c| match c {
                    '{' => Some(true),
                    '[' => Some(false),
                    _ => None,
                });
                handle_rpc_call(
                    request.as_bytes(),
                    is_single.unwrap_or(true),
                    BatchRequestConfig::Unlimited,
                    &rpc_service,
                )
                .await
            };
            drop(conn);

            // Notifications get no answer, and a subscription already sent its own.
            (!rp.is_notification() && !rp.is_subscription()).then(|| rp.into_json())
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

struct ProcessConnection<RpcMiddleware> {
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
    conn_permit: Arc<OwnedSemaphorePermit>,
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
fn process_connection<RpcMiddleware>(params: ProcessConnection<RpcMiddleware>)
where
    RpcMiddleware: Layer<RpcService> + Send + 'static,
    RpcMiddleware::Service: RpcServiceT + Send + Sync + 'static,
{
    let ProcessConnection {
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

    let codec =
        StreamCodec::stream_incoming().with_max_length(server_cfg.max_request_body_size as usize);
    let ipc = IpcConn(tokio_util::codec::Decoder::framed(codec, local_socket_stream));

    let (tx, rx) = mpsc::channel::<String>(MESSAGE_BUFFER_CAPACITY);
    let connection = Connection::new(
        ConnectionId(conn_id as usize),
        tx,
        server_cfg.max_subscriptions_per_connection,
        id_provider,
    );
    let service = TowerServiceNoHttp {
        inner: ServiceData { methods, conn_permit, connection, server_cfg },
        rpc_middleware,
    };

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
                let Some(item) = item else { break };
                conn.push_back(item);
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
    /// Maximum number of incoming connections allowed.
    max_connections: u32,
    /// Maximum number of subscriptions per connection.
    max_subscriptions_per_connection: u32,
    /// Custom tokio runtime to run the server on.
    tokio_runtime: Option<tokio::runtime::Handle>,
    /// The permissions to create the IPC socket with.
    ipc_socket_permissions: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_request_body_size: DEFAULT_SIZE_LIMIT,
            max_response_body_size: DEFAULT_SIZE_LIMIT,
            max_connections: 100,
            max_subscriptions_per_connection: 1024,
            tokio_runtime: None,
            ipc_socket_permissions: None,
        }
    }
}

/// Builder to configure and create a JSON-RPC server
#[derive(Debug)]
pub struct Builder<RpcMiddleware> {
    settings: Settings,
    /// Subscription ID provider.
    id_provider: Arc<dyn IdProvider>,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
}

impl Default for Builder<Identity> {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            id_provider: Arc::new(RandomIntegerIdProvider),
            rpc_middleware: RpcServiceBuilder::new(),
        }
    }
}

impl<RpcMiddleware> Builder<RpcMiddleware> {
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

    /// Configure a custom [`tokio::runtime::Handle`] to run the server on.
    ///
    /// Default: [`tokio::spawn`]
    pub fn custom_tokio_runtime(mut self, rt: tokio::runtime::Handle) -> Self {
        self.settings.tokio_runtime = Some(rt);
        self
    }

    /// Sets the permissions for the IPC socket file.
    ///
    /// On Unix, accepts an octal mode up to `0o7777` with an optional `0o` prefix. Invalid modes or
    /// failures to apply them cause [`IpcServer::start`] to return an error. This includes
    /// platforms that do not support setting socket permissions before binding. `None` uses the
    /// system's umask. Ignored on non-Unix platforms.
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
    /// use alloy_rpc_server::RandomStringIdProvider;
    /// use reth_ipc::server::Builder;
    ///
    /// let builder = Builder::default().set_id_provider(RandomStringIdProvider::new(16));
    /// ```
    pub fn set_id_provider<I: IdProvider + 'static>(mut self, id_provider: I) -> Self {
        self.id_provider = Arc::new(id_provider);
        self
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
    pub fn set_rpc_middleware<T>(self, rpc_middleware: RpcServiceBuilder<T>) -> Builder<T> {
        Builder { settings: self.settings, id_provider: self.id_provider, rpc_middleware }
    }

    /// Finalize the configuration of the server. Consumes the [`Builder`].
    pub fn build(self, endpoint: String) -> IpcServer<RpcMiddleware> {
        IpcServer {
            endpoint,
            cfg: self.settings,
            id_provider: self.id_provider,
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
    use alloy_rpc_server::{
        error::TOO_MANY_SUBSCRIPTIONS_CODE, Batch, BatchEntry, ErrorPayload,
        PendingSubscriptionSink, Request, RpcModule, SubscriptionMessage,
    };
    use futures::future::select;
    use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
    use reth_tracing::init_test_tracing;
    use serde_json::{json, Value};
    use std::pin::pin;
    use tokio::sync::broadcast;
    use tokio_stream::wrappers::BroadcastStream;
    use tokio_util::codec::FramedRead;

    /// Raw JSON-RPC client that talks to the server over the IPC socket.
    struct TestClient {
        send: SendHalf,
        recv: FramedRead<RecvHalf, StreamCodec>,
        next_id: u64,
    }

    impl TestClient {
        async fn connect(endpoint: &str) -> Self {
            let name = endpoint.to_fs_name::<GenericFilePath>().unwrap();
            let (recv, send) = LocalSocketStream::connect(name).await.unwrap().split();
            Self { send, recv: FramedRead::new(recv, StreamCodec::stream_incoming()), next_id: 0 }
        }

        /// Sends a call and returns its response, or `None` if the server closed the
        /// connection.
        async fn call(&mut self, method: &str, params: Value) -> Option<Value> {
            let request = self.request(method, params);
            self.send(request).await.ok()?;
            self.recv().await
        }

        /// Returns the request object for a call with a fresh id.
        fn request(&mut self, method: &str, params: Value) -> Value {
            self.next_id += 1;
            json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        }

        async fn send(&mut self, msg: Value) -> io::Result<()> {
            self.send.write_all(msg.to_string().as_bytes()).await
        }

        /// Returns the next message from the server, or `None` if the connection is closed.
        async fn recv(&mut self) -> Option<Value> {
            let msg = self.recv.next().await?.ok()?;
            Some(serde_json::from_str(&msg).unwrap())
        }
    }

    /// Returns the error code of a response, or `None` if it succeeded.
    fn error_code(response: &Value) -> Option<i64> {
        response.get("error").map(|err| err["code"].as_i64().unwrap())
    }

    #[cfg(unix)]
    use std::{os::unix::net::UnixListener, path::Path, time::Duration};

    #[tokio::test]
    #[cfg(unix)]
    async fn test_ipc_socket_permissions() {
        for (permissions, mode) in
            [("0777", 0o777), ("600", 0o600), ("0600", 0o600), ("0o600", 0o600)]
        {
            let endpoint = dummy_name();
            let server = Builder::default()
                .set_ipc_socket_permissions(Some(permissions.to_string()))
                .build(endpoint.clone());
            match server.start(RpcModule::new(())).await {
                Ok(handle) if !cfg!(target_os = "macos") => {
                    let actual = std::fs::metadata(&endpoint).unwrap().permissions().mode();
                    assert_eq!(actual & 0o777, mode);
                    handle.stop().unwrap();
                    tokio::time::timeout(Duration::from_secs(5), handle.stopped()).await.unwrap();
                }
                Err(error)
                    if !cfg!(any(
                        target_os = "linux",
                        target_os = "android",
                        target_os = "openbsd"
                    )) =>
                {
                    assert_eq!(error.source.kind(), io::ErrorKind::Unsupported, "{mode:o}");
                    assert_eq!(error.endpoint, endpoint);
                    assert!(!Path::new(&endpoint).exists());
                }
                result => panic!("Unexpected IPC startup result: {result:?}"),
            }
        }
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
        module
            .register_method("anything", |(): (), _| Ok::<_, ErrorPayload>("a".repeat(101)))
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let response = client.call("anything", json!([])).await.unwrap();
        assert!(response["error"]["data"].as_str().unwrap().contains("Exceeded max limit of"));
    }

    #[tokio::test]
    async fn can_set_the_max_request_body_size() {
        init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().max_request_body_size(100).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |(): (), _| Ok::<_, ErrorPayload>("succeed")).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let response = client.call("anything", json!(["a".repeat(101)])).await;
        assert!(response.is_none_or(|response| error_code(&response).is_some()));

        // The batch is more than 100 bytes.
        let mut client = TestClient::connect(endpoint).await;
        let batch = (0..3).map(|_| client.request("anything", json!([]))).collect();
        client.send(Value::Array(batch)).await.unwrap();
        let response = client.recv().await;
        assert!(response.is_none_or(|response| error_code(&response).is_some()));
    }

    #[tokio::test]
    async fn can_set_max_connections() {
        init_test_tracing();

        let endpoint = &dummy_name();
        let server = Builder::default().max_connections(2).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |(): (), _| Ok::<_, ErrorPayload>("succeed")).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client1 = TestClient::connect(endpoint).await;
        let mut client2 = TestClient::connect(endpoint).await;
        let mut client3 = TestClient::connect(endpoint).await;

        assert_eq!(client1.call("anything", json!([])).await.unwrap()["result"], "succeed");
        assert_eq!(client2.call("anything", json!([])).await.unwrap()["result"], "succeed");
        // Third connection is rejected
        assert!(client3.call("anything", json!([])).await.is_none());

        // Decrement connection count
        drop(client2);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Can connect again
        let mut client4 = TestClient::connect(endpoint).await;
        assert_eq!(client4.call("anything", json!([])).await.unwrap()["result"], "succeed");
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
            .register_async_method("hang", |(): (), ctx| async move {
                // `dropped_tx` is dropped together with the call
                let (started_tx, _dropped_tx) = ctx.lock().unwrap().take().unwrap();
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
                Ok::<_, ErrorPayload>("unreachable")
            })
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        tokio::select! {
            _ = client.call("hang", json!([])) => panic!("call completed"),
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
            .register_async_method("maybe_panic", |(panic,): (bool,), _| async move {
                assert!(!panic, "requested panic");
                Ok::<_, ErrorPayload>("ok")
            })
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let response = client.call("maybe_panic", json!([true])).await.unwrap();
        assert_eq!(error_code(&response), Some(ErrorCode::InternalError.code()), "{response}");

        let batch = vec![
            client.request("maybe_panic", json!([true])),
            client.request("maybe_panic", json!([false])),
        ];
        client.send(Value::Array(batch)).await.unwrap();
        let responses = client.recv().await.unwrap();
        assert_eq!(error_code(&responses[0]), Some(ErrorCode::InternalError.code()));
        assert_eq!(responses[1]["result"], "ok");
    }

    #[tokio::test]
    async fn test_invalid_id_is_not_a_notification() {
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |(): (), _| Ok::<_, ErrorPayload>("ok")).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        client.send(json!({"jsonrpc": "2.0", "method": "anything", "id": {}})).await.unwrap();
        // The message used to be treated as a notification and left unanswered.
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), client.recv())
            .await
            .expect("message with an invalid id was not answered")
            .unwrap();
        assert_eq!(
            response,
            json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": ErrorCode::InvalidRequest.code(), "message": "Invalid request"},
            })
        );
    }

    #[tokio::test]
    async fn test_rpc_request() {
        init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        let msg = r#"{"jsonrpc":"2.0","id":83,"result":"0x7a69"}"#;
        module.register_method("eth_chainId", move |(): (), _| Ok::<_, ErrorPayload>(msg)).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let response = client.call("eth_chainId", json!([])).await.unwrap();
        assert_eq!(response["result"], msg);
    }

    #[tokio::test]
    async fn test_batch_request() {
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |(): (), _| Ok::<_, ErrorPayload>("ok")).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let batch = (0..3).map(|_| client.request("anything", json!([]))).collect();
        client.send(Value::Array(batch)).await.unwrap();
        let responses = client.recv().await.unwrap();
        let results =
            responses.as_array().unwrap().iter().map(|r| &r["result"]).collect::<Vec<_>>();
        assert_eq!(results, vec!["ok", "ok", "ok"]);
    }

    #[tokio::test]
    async fn test_ipc_modules() {
        reth_tracing::init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        let msg = r#"{"admin":"1.0","debug":"1.0","engine":"1.0","eth":"1.0","ethash":"1.0","miner":"1.0","net":"1.0","rpc":"1.0","txpool":"1.0","web3":"1.0"}"#;
        module.register_method("rpc_modules", move |(): (), _| Ok::<_, ErrorPayload>(msg)).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let response = client.call("rpc_modules", json!([])).await.unwrap();
        assert_eq!(response["result"], msg);
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
                |(): (), pending, tx| async move {
                    let rx = tx.subscribe();
                    let stream = BroadcastStream::new(rx);
                    pipe_from_stream_with_bounded_buffer(pending, stream).await?;
                    Ok(())
                },
            )
            .unwrap();

        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let response = client.call("subscribe_hello", json!([])).await.unwrap();
        let sub_id = response["result"].clone();

        for _ in 0..16 {
            let notification = client.recv().await.unwrap();
            assert_eq!(notification["method"], "s_hello");
            assert_eq!(notification["params"]["subscription"], sub_id);
            assert!(notification["params"]["result"].is_u64());
        }
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
                |(): (), pending, _| async move {
                    let Ok(sink) = pending.accept().await else { return Ok(()) };
                    sink.closed().await;
                    Ok(())
                },
            )
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let response = client.call("subscribe_hello", json!([])).await.unwrap();
        assert_eq!(error_code(&response), None, "{response}");
        let response = client.call("subscribe_hello", json!([])).await.unwrap();
        assert_eq!(error_code(&response), Some(TOO_MANY_SUBSCRIPTIONS_CODE), "{response}");
    }

    #[tokio::test]
    async fn test_rpc_middleware() {
        #[derive(Clone)]
        struct ModifyRequestIf<S>(S);

        impl<S> RpcServiceT for ModifyRequestIf<S>
        where
            S: Send + Sync + RpcServiceT,
        {
            fn call<'a>(
                &self,
                mut req: Request<'a>,
            ) -> impl Future<Output = MethodResponse> + Send + 'a {
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
            ) -> impl Future<Output = MethodResponse> + Send + 'a {
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
                mut n: Request<'a>,
            ) -> impl Future<Output = MethodResponse> + Send + 'a {
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
        module
            .register_method("say_hello", move |(): (), _| Ok::<_, ErrorPayload>(hello_msg))
            .unwrap();
        module
            .register_method("say_goodbye", move |(): (), _| Ok::<_, ErrorPayload>(goodbye_msg))
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let mut client = TestClient::connect(endpoint).await;
        let say_hello_response = client.call("say_hello", json!([])).await.unwrap();
        let say_goodbye_response = client.call("say_goodbye", json!([])).await.unwrap();

        assert_eq!(say_hello_response["result"], goodbye_msg);
        assert_eq!(say_goodbye_response["result"], hello_msg);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn invalid_ipc_socket_permissions() {
        for permissions in
            ["not-octal", "", "0o", "888", "0o0o600", "60o00", "100000000000", "10000"]
        {
            let endpoint = dummy_name();
            let server = Builder::default()
                .set_ipc_socket_permissions(Some(permissions.to_string()))
                .build(endpoint.clone());

            let error = server.start(RpcModule::new(())).await.unwrap_err();
            assert_eq!(error.source.kind(), io::ErrorKind::InvalidInput, "{permissions}");
            assert_eq!(error.endpoint, endpoint);
            assert!(!Path::new(&endpoint).exists());
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn invalid_ipc_permissions_preserve_existing_endpoint() {
        for (permissions, kind) in [
            ("not-octal", io::ErrorKind::InvalidInput),
            #[cfg(target_os = "macos")]
            ("0600", io::ErrorKind::Unsupported),
        ] {
            let endpoint = dummy_name();
            let _listener = ListenerOptions::new()
                .name(endpoint.as_str().to_fs_name::<GenericFilePath>().unwrap())
                .create_tokio()
                .unwrap();
            let server = Builder::default()
                .set_ipc_socket_permissions(Some(permissions.to_string()))
                .build(endpoint.clone());

            let error = server.start(RpcModule::new(())).await.unwrap_err();
            assert_eq!(error.source.kind(), kind, "{permissions}");
            assert_eq!(error.endpoint, endpoint);
            assert!(Path::new(&endpoint).exists());
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn replaces_stale_ipc_endpoint() {
        let endpoint = dummy_name();
        drop(UnixListener::bind(&endpoint).unwrap());
        assert!(Path::new(&endpoint).exists());

        let handle =
            Builder::default().build(endpoint.clone()).start(RpcModule::new(())).await.unwrap();
        handle.stop().unwrap();
        tokio::time::timeout(Duration::from_secs(5), handle.stopped()).await.unwrap();
    }
}
