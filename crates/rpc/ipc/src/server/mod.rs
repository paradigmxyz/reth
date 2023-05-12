//! JSON-RPC IPC server implementation

use crate::server::{
    connection::{Incoming, IpcConn, JsonRpcStream},
    future::{ConnectionGuard, FutureDriver, StopHandle},
};
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use jsonrpsee::{
    core::{Error, TEN_MB_SIZE_BYTES},
    server::{logger::Logger, IdProvider, RandomIntegerIdProvider, ServerHandle},
    Methods,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{oneshot, watch, OwnedSemaphorePermit},
};
use tower::{layer::util::Identity, Service};
use tracing::{trace, warn};

// re-export so can be used during builder setup
pub use parity_tokio_ipc::Endpoint;

mod connection;
mod future;
mod ipc;

/// Ipc Server implementation

// This is an adapted `jsonrpsee` Server, but for `Ipc` connections.
pub struct IpcServer<B = Identity, L = ()> {
    /// The endpoint we listen for incoming transactions
    endpoint: Endpoint,
    logger: L,
    id_provider: Arc<dyn IdProvider>,
    cfg: Settings,
    service_builder: tower::ServiceBuilder<B>,
}

impl IpcServer {
    /// Returns the configured [Endpoint]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Start responding to connections requests.
    ///
    /// This will run on the tokio runtime until the server is stopped or the ServerHandle is
    /// dropped.
    ///
    /// ```
    /// use jsonrpsee::RpcModule;
    /// use reth_ipc::server::Builder;
    /// async fn run_server() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ///     let server = Builder::default().build("/tmp/my-uds")?;
    ///     let mut module = RpcModule::new(());
    ///     module.register_method("say_hello", |_, _| "lo")?;
    ///     let handle = server.start(module).await?;
    ///
    ///     // In this example we don't care about doing shutdown so let's it run forever.
    ///     // You may use the `ServerHandle` to shut it down or manage it yourself.
    ///     let server = tokio::spawn(handle.stopped());
    ///     server.await.unwrap();
    ///     Ok(())
    /// }
    /// ```
    pub async fn start(mut self, methods: impl Into<Methods>) -> Result<ServerHandle, Error> {
        let methods = methods.into();
        let (stop_tx, stop_rx) = watch::channel(());

        let stop_handle = StopHandle::new(stop_rx);

        // use a signal channel to wait until we're ready to accept connections
        let (tx, rx) = oneshot::channel();

        match self.cfg.tokio_runtime.take() {
            Some(rt) => rt.spawn(self.start_inner(methods, stop_handle, tx)),
            None => tokio::spawn(self.start_inner(methods, stop_handle, tx)),
        };
        rx.await.expect("channel is open").map_err(Error::Custom)?;

        Ok(ServerHandle::new(stop_tx))
    }

    #[allow(clippy::let_unit_value)]
    async fn start_inner(
        self,
        methods: Methods,
        stop_handle: StopHandle,
        on_ready: oneshot::Sender<Result<(), String>>,
    ) -> io::Result<()> {
        trace!( endpoint=?self.endpoint.path(), "starting ipc server" );

        if cfg!(unix) {
            // ensure the file does not exist
            if std::fs::remove_file(self.endpoint.path()).is_ok() {
                warn!( endpoint=?self.endpoint.path(), "removed existing file");
            }
        }

        let max_request_body_size = self.cfg.max_request_body_size;
        let max_response_body_size = self.cfg.max_response_body_size;
        let max_log_length = self.cfg.max_log_length;
        let id_provider = self.id_provider;
        let max_subscriptions_per_connection = self.cfg.max_subscriptions_per_connection;
        let logger = self.logger;

        let mut id: u32 = 0;
        let connection_guard = ConnectionGuard::new(self.cfg.max_connections as usize);

        let mut connections = FutureDriver::default();
        let incoming = match self.endpoint.incoming() {
            Ok(connections) => Incoming::new(connections),
            Err(err) => {
                on_ready.send(Err(err.to_string())).ok();
                return Err(err)
            }
        };
        // signal that we're ready to accept connections
        on_ready.send(Ok(())).ok();

        let mut incoming = Monitored::new(incoming, &stop_handle);

        trace!("accepting ipc connections");
        loop {
            match connections.select_with(&mut incoming).await {
                Ok(ipc) => {
                    trace!("established new connection");
                    let conn = match connection_guard.try_acquire() {
                        Some(conn) => conn,
                        None => {
                            warn!("Too many connections. Please try again later.");
                            connections.add(ipc.reject_connection().boxed());
                            continue
                        }
                    };

                    let tower_service = TowerService {
                        inner: ServiceData {
                            methods: methods.clone(),
                            max_request_body_size,
                            max_response_body_size,
                            max_log_length,
                            id_provider: id_provider.clone(),
                            stop_handle: stop_handle.clone(),
                            max_subscriptions_per_connection,
                            conn_id: id,
                            logger,
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
            .finish()
    }
}

/// Data required by the server to handle requests.
#[derive(Debug, Clone)]
#[allow(unused)]
pub(crate) struct ServiceData<L: Logger> {
    /// Registered server methods.
    pub(crate) methods: Methods,
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
/// This is similar to [`hyper::service::service_fn`](https://docs.rs/hyper/latest/hyper/service/fn.service_fn.html).
#[derive(Debug)]
pub struct TowerService<L: Logger> {
    inner: ServiceData<L>,
}

impl<L: Logger> Service<String> for TowerService<L> {
    type Response = String;

    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    /// Opens door for back pressure implementation.
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: String) -> Self::Future {
        trace!("{:?}", request);

        // handle the request
        let data = ipc::HandleRequest {
            methods: self.inner.methods.clone(),
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
    S: Service<String, Response = String> + Send + 'static,
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
                warn!("Failed to send response: {:?}", err);
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
    logger: L,
    /// Subscription ID provider.
    id_provider: Arc<dyn IdProvider>,
    service_builder: tower::ServiceBuilder<B>,
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            settings: Settings::default(),
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

    /// Add a logger to the builder [`Logger`].
    pub fn set_logger<T: Logger>(self, logger: T) -> Builder<B, T> {
        Builder {
            settings: self.settings,
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
    /// #[tokio::main]
    /// async fn main() {
    ///     let builder = tower::ServiceBuilder::new();
    ///
    ///     let server = reth_ipc::server::Builder::default()
    ///         .set_middleware(builder)
    ///         .build("/tmp/my-uds")
    ///         .unwrap();
    /// }
    /// ```
    pub fn set_middleware<T>(self, service_builder: tower::ServiceBuilder<T>) -> Builder<T, L> {
        Builder {
            settings: self.settings,
            logger: self.logger,
            id_provider: self.id_provider,
            service_builder,
        }
    }

    /// Finalize the configuration of the server. Consumes the [`Builder`].
    pub fn build(self, endpoint: impl AsRef<str>) -> Result<IpcServer<B, L>, Error> {
        let endpoint = Endpoint::new(endpoint.as_ref().to_string());
        self.build_with_endpoint(endpoint)
    }

    /// Finalize the configuration of the server. Consumes the [`Builder`].
    pub fn build_with_endpoint(self, endpoint: Endpoint) -> Result<IpcServer<B, L>, Error> {
        Ok(IpcServer {
            endpoint,
            cfg: self.settings,
            logger: self.logger,
            id_provider: self.id_provider,
            service_builder: self.service_builder,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::IpcClientBuilder;
    use jsonrpsee::{core::client::ClientT, rpc_params, RpcModule};
    use parity_tokio_ipc::dummy_endpoint;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_rpc_request() {
        let endpoint = dummy_endpoint();
        let server = Builder::default().build(&endpoint).unwrap();
        let mut module = RpcModule::new(());
        let msg = r#"{"jsonrpc":"2.0","id":83,"result":"0x7a69"}"#;
        module.register_method("eth_chainId", move |_, _| msg).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: String = client.request("eth_chainId", rpc_params![]).await.unwrap();
        assert_eq!(response, msg);
    }
}
