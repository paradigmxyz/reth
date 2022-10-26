//! JSON-RPC IPC server implementation

use jsonrpsee::{
    core::{
        server::{resource_limiting::Resources, rpc_module::Methods},
        Error, TEN_MB_SIZE_BYTES,
    },
    server::{logger::Logger, IdProvider, RandomIntegerIdProvider, ServerBuilder, ServerHandle},
};
use parity_tokio_ipc::Endpoint;
use std::sync::Arc;
use tokio::sync::watch;
use tower::layer::util::Identity;

mod connection;

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

    async fn start_inner(self, methods: Methods, stop_handle: StopHandle) {}
}

impl std::fmt::Debug for IpcServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcServer")
            .field("endpoint", &"{{}}")
            .field("cfg", &self.cfg)
            .field("id_provider", &self.id_provider)
            .field("resources", &self.resources)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StopHandle(watch::Receiver<()>);

impl StopHandle {
    pub(crate) fn new(rx: watch::Receiver<()>) -> Self {
        Self(rx)
    }

    pub(crate) fn shutdown_requested(&self) -> bool {
        // if a message has been seen, it means that `stop` has been called.
        self.0.has_changed().unwrap_or(true)
    }

    pub(crate) async fn shutdown(&mut self) {
        // Err(_) implies that the `sender` has been dropped.
        // Ok(_) implies that `stop` has been called.
        let _ = self.0.changed().await;
    }
}

/// JSON-RPC IPC server settings.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Maximum size in bytes of a request.
    max_request_body_size: u32,
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
