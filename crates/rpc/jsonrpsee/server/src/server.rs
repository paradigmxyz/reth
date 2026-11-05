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

use std::future::Future;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::task::Poll;
use std::time::Duration;

use crate::future::{ConnectionGuard, ServerHandle, SessionClose, SessionClosedFuture, StopHandle, session_close};
use crate::middleware::rpc::{RpcService, RpcServiceCfg};
use crate::transport::ws::BackgroundTaskParams;
use crate::transport::{http, ws};
use crate::utils::{DEFAULT_FIRST_REQUEST_TIMEOUT, NotifyOnRequest, deserialize_with_ext, no_request_within};
use crate::{Extensions, HttpBody, HttpRequest, HttpResponse, LOG_TARGET};

use futures_util::future::{Either, FutureExt};
use futures_util::io::{BufReader, BufWriter};
use hyper::body::Bytes;
use hyper_util::rt::{TokioExecutor, TokioIo};
use jsonrpsee_core::id_providers::RandomIntegerIdProvider;
use jsonrpsee_core::middleware::{Batch, BatchEntry, BatchEntryErr, RpcServiceBuilder, RpcServiceT};
use jsonrpsee_core::server::helpers::prepare_error;
use jsonrpsee_core::server::{BoundedSubscriptions, ConnectionId, MethodResponse, MethodSink, Methods};
use jsonrpsee_core::traits::IdProvider;
use jsonrpsee_core::{BoxError, JsonRawValue, TEN_MB_SIZE_BYTES};
use jsonrpsee_types::error::{
	BATCHES_NOT_SUPPORTED_CODE, BATCHES_NOT_SUPPORTED_MSG, ErrorCode, reject_too_big_batch_request,
};
use jsonrpsee_types::{ErrorObject, Id};
use soketto::handshake::http::is_upgrade_request;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::{Notify, OwnedSemaphorePermit, mpsc, watch};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tower::layer::util::Identity;
use tower::{Layer, Service};
use tracing::{Instrument, instrument};

/// Default maximum connections allowed.
const MAX_CONNECTIONS: u32 = 100;

type Notif<'a> = Option<std::borrow::Cow<'a, JsonRawValue>>;

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
		f.debug_struct("Server").field("listener", &self.listener).field("server_cfg", &self.server_cfg).finish()
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
	<<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service as Service<HttpRequest>>::Future: Send,
	Body: http_body::Body<Data = Bytes> + Send + 'static,
	<Body as http_body::Body>::Error: Into<BoxError>,
{
	/// Start responding to connections requests.
	///
	/// This will run on the tokio runtime until the server is stopped or the `ServerHandle` is dropped.
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
		let mut id: u32 = 0;
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
						conn_id: id,
						server_cfg: self.server_cfg.clone(),
						conn_guard: &connection_guard,
						socket,
						drop_on_completion: drop_on_completion.clone(),
					});
					id = id.wrapping_add(1);
					stopped = stop;
				}
				AcceptConnection::Err((e, stop)) => {
					tracing::debug!(target: LOG_TARGET, "Error while awaiting a new connection: {:?}", e);
					stopped = stop;
				}
				AcceptConnection::Shutdown => break,
			}
		}

		// Drop the last Sender
		drop(drop_on_completion);

		// Once this channel is closed it is safe to assume that all connections have been gracefully shutdown
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
	/// Ping settings.
	pub(crate) ping_config: Option<PingConfig>,
	/// ID provider.
	pub(crate) id_provider: Arc<dyn IdProvider>,
	/// `TCP_NODELAY` settings.
	pub(crate) tcp_no_delay: bool,
	/// `KEEP_ALIVE` duration.
	pub(crate) keep_alive: Option<std::time::Duration>,
	/// `KEEP_ALIVE_TIMEOUT` duration.
	pub(crate) keep_alive_timeout: Duration,
	/// Timeout for receiving the first request of a connection.
	pub(crate) first_request_timeout: Option<Duration>,
}

/// The builder to configure and create a JSON-RPC server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfigBuilder {
	/// Maximum size in bytes of a request.
	max_request_body_size: u32,
	/// Maximum size in bytes of a response.
	max_response_body_size: u32,
	/// Maximum number of incoming connections allowed.
	max_connections: u32,
	/// Maximum number of subscriptions per connection.
	max_subscriptions_per_connection: u32,
	/// Whether batch requests are supported by this server or not.
	batch_requests_config: BatchRequestConfig,
	/// Custom tokio runtime to run the server on.
	tokio_runtime: Option<tokio::runtime::Handle>,
	/// Enable HTTP.
	enable_http: bool,
	/// Enable WS.
	enable_ws: bool,
	/// Number of messages that server is allowed to `buffer` until backpressure kicks in.
	message_buffer_capacity: u32,
	/// Ping settings.
	ping_config: Option<PingConfig>,
	/// ID provider.
	id_provider: Arc<dyn IdProvider>,
	/// `TCP_NODELAY` settings.
	tcp_no_delay: bool,
	/// `KEEP_ALIVE` duration.
	keep_alive: Option<std::time::Duration>,
	/// `KEEP_ALIVE_TIMEOUT` duration.
	keep_alive_timeout: std::time::Duration,
	/// Timeout for receiving the first request of a connection.
	first_request_timeout: Option<Duration>,
}

/// Builder for [`TowerService`].
#[derive(Debug, Clone)]
pub struct TowerServiceBuilder<RpcMiddleware, HttpMiddleware> {
	/// ServerConfig
	pub(crate) server_cfg: ServerConfig,
	/// RPC middleware.
	pub(crate) rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
	/// HTTP middleware.
	pub(crate) http_middleware: tower::ServiceBuilder<HttpMiddleware>,
	/// Connection ID.
	pub(crate) conn_id: Arc<AtomicU32>,
	/// Connection guard.
	pub(crate) conn_guard: ConnectionGuard,
}

/// Configuration for batch request handling.
#[derive(Debug, Copy, Clone)]
pub enum BatchRequestConfig {
	/// Batch requests are disabled.
	Disabled,
	/// Each batch request is limited to `len` and any batch request bigger than `len` will not be processed.
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
	pub(crate) conn_id: u32,
	/// Connection guard.
	pub(crate) _conn_permit: Arc<OwnedSemaphorePermit>,
}

impl ConnectionState {
	/// Create a new connection state.
	pub fn new(stop_handle: StopHandle, conn_id: u32, conn_permit: OwnedSemaphorePermit) -> ConnectionState {
		Self { stop_handle, conn_id, _conn_permit: Arc::new(conn_permit) }
	}
}

/// Configuration for WebSocket ping/pong mechanism and it may be used to disconnect
/// an inactive connection.
///
/// jsonrpsee doesn't associate the ping/pong frames just that if
/// a pong frame isn't received within the `inactive_limit` then it's regarded
/// as missed.
///
/// Such that the `inactive_limit` should be configured to longer than a single
/// WebSocket ping takes or it might be missed and may end up
/// terminating the connection.
///
/// Default: ping_interval: 30 seconds, max failures: 1 and inactive limit: 40 seconds.
#[derive(Debug, Copy, Clone)]
pub struct PingConfig {
	/// Period which the server pings the connected client.
	pub(crate) ping_interval: Duration,
	/// Max allowed time for a connection to stay idle.
	pub(crate) inactive_limit: Duration,
	/// Max failures.
	pub(crate) max_failures: usize,
}

impl Default for PingConfig {
	fn default() -> Self {
		Self { ping_interval: Duration::from_secs(30), max_failures: 1, inactive_limit: Duration::from_secs(40) }
	}
}

impl PingConfig {
	/// Create a new PingConfig.
	pub fn new() -> Self {
		Self::default()
	}

	/// Configure the interval when the WebSocket pings are sent out.
	pub fn ping_interval(mut self, ping_interval: Duration) -> Self {
		self.ping_interval = ping_interval;
		self
	}

	/// Configure how long to wait for the WebSocket pong.
	/// When this limit is expired it's regarded as the client is unresponsive.
	///
	/// You may configure how many times the client is allowed to be "inactive" by
	/// [`PingConfig::max_failures`].
	pub fn inactive_limit(mut self, inactivity_limit: Duration) -> Self {
		self.inactive_limit = inactivity_limit;
		self
	}

	/// Configure how many times the remote peer is allowed be
	/// inactive until the connection is closed.
	///
	/// # Panics
	///
	/// This method panics if `max` == 0.
	pub fn max_failures(mut self, max: usize) -> Self {
		assert!(max > 0);
		self.max_failures = max;
		self
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
		ServerConfigBuilder {
			max_request_body_size: TEN_MB_SIZE_BYTES,
			max_response_body_size: TEN_MB_SIZE_BYTES,
			max_connections: MAX_CONNECTIONS,
			max_subscriptions_per_connection: 1024,
			batch_requests_config: BatchRequestConfig::Unlimited,
			tokio_runtime: None,
			enable_http: true,
			enable_ws: true,
			message_buffer_capacity: 1024,
			ping_config: None,
			id_provider: Arc::new(RandomIntegerIdProvider),
			tcp_no_delay: true,
			keep_alive: None,
			//same as `hyper` default
			keep_alive_timeout: Duration::from_secs(20),
			first_request_timeout: Some(DEFAULT_FIRST_REQUEST_TIMEOUT),
		}
	}
}

impl ServerConfigBuilder {
	/// Create a new [`ServerConfigBuilder`].
	pub fn new() -> Self {
		Self::default()
	}

	/// Set the maximum size of a request body in bytes. Default is 10 MiB.
	pub fn max_request_body_size(mut self, size: u32) -> Self {
		self.max_request_body_size = size;
		self
	}

	/// Set the maximum size of a response body in bytes. Default is 10 MiB.
	pub fn max_response_body_size(mut self, size: u32) -> Self {
		self.max_response_body_size = size;
		self
	}

	/// Set the maximum number of connections allowed. Default is 100.
	pub fn max_connections(mut self, max: u32) -> Self {
		self.max_connections = max;
		self
	}

	/// Set the maximum number of connections allowed. Default is 1024.
	pub fn max_subscriptions_per_connection(mut self, max: u32) -> Self {
		self.max_subscriptions_per_connection = max;
		self
	}

	/// Configure how [batch requests](https://www.jsonrpc.org/specification#batch) shall be handled
	/// by the server.
	///
	/// Default: batch requests are allowed and can be arbitrary big but the maximum payload size is limited.
	pub fn set_batch_request_config(mut self, cfg: BatchRequestConfig) -> Self {
		self.batch_requests_config = cfg;
		self
	}

	/// Configure a custom [`tokio::runtime::Handle`] to run the server on.
	///
	/// Default: [`tokio::spawn`]
	pub fn custom_tokio_runtime(mut self, rt: tokio::runtime::Handle) -> Self {
		self.tokio_runtime = Some(rt);
		self
	}

	/// Configure the server to only serve JSON-RPC HTTP requests.
	///
	/// Default: both http and ws are enabled.
	pub fn http_only(mut self) -> Self {
		self.enable_http = true;
		self.enable_ws = false;
		self
	}

	/// Configure the server to only serve JSON-RPC WebSocket requests.
	///
	/// That implies that server just denies HTTP requests which isn't a WebSocket upgrade request
	///
	/// Default: both http and ws are enabled.
	pub fn ws_only(mut self) -> Self {
		self.enable_http = false;
		self.enable_ws = true;
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
	///
	pub fn set_message_buffer_capacity(mut self, c: u32) -> Self {
		assert!(c > 0, "buffer capacity must be set to > 0");
		self.message_buffer_capacity = c;
		self
	}

	/// Enable WebSocket ping/pong on the server.
	///
	/// Default: pings are disabled.
	///
	/// # Examples
	///
	/// ```rust
	/// use std::{time::Duration, num::NonZeroUsize};
	/// use jsonrpsee_server::{ServerConfigBuilder, PingConfig};
	///
	/// // Set the ping interval to 10 seconds but terminates the connection if a client is inactive for more than 2 minutes
	/// let ping_cfg = PingConfig::new().ping_interval(Duration::from_secs(10)).inactive_limit(Duration::from_secs(60 * 2));
	/// let builder = ServerConfigBuilder::default().enable_ws_ping(ping_cfg);
	/// ```
	pub fn enable_ws_ping(mut self, config: PingConfig) -> Self {
		self.ping_config = Some(config);
		self
	}

	/// Disable WebSocket ping/pong on the server.
	///
	/// Default: pings are disabled.
	pub fn disable_ws_ping(mut self) -> Self {
		self.ping_config = None;
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
	/// use jsonrpsee_server::{ServerConfigBuilder, RandomStringIdProvider, IdProvider};
	///
	/// // static dispatch
	/// let builder1 = ServerConfigBuilder::default().set_id_provider(RandomStringIdProvider::new(16));
	///
	/// // or dynamic dispatch
	/// let builder2 = ServerConfigBuilder::default().set_id_provider(Box::new(RandomStringIdProvider::new(16)));
	/// ```
	///
	pub fn set_id_provider<I: IdProvider + 'static>(mut self, id_provider: I) -> Self {
		self.id_provider = Arc::new(id_provider);
		self
	}

	/// Configure `TCP_NODELAY` on the socket to the supplied value `nodelay`.
	///
	/// Default is `true`.
	pub fn set_tcp_no_delay(mut self, no_delay: bool) -> Self {
		self.tcp_no_delay = no_delay;
		self
	}

	/// Configure `KEEP_ALIVE` hyper to the supplied value `keep_alive`.
	pub fn set_keep_alive(mut self, keep_alive: Option<std::time::Duration>) -> Self {
		self.keep_alive = keep_alive;
		self
	}

	/// Configure `KEEP_ALIVE_TIMEOUT` hyper to the supplied value `keep_alive_timeout`.
	pub fn set_keep_alive_timeout(mut self, keep_alive_timeout: Duration) -> Self {
		self.keep_alive_timeout = keep_alive_timeout;
		self
	}

	/// Configure how long a connection may take to send its first request, e.g. the WebSocket
	/// upgrade request, before it is closed. `None` disables the timeout.
	///
	/// Default is 30 seconds.
	pub fn set_first_request_timeout(mut self, first_request_timeout: Option<Duration>) -> Self {
		self.first_request_timeout = first_request_timeout;
		self
	}

	/// Build the [`ServerConfig`].
	pub fn build(self) -> ServerConfig {
		ServerConfig {
			max_request_body_size: self.max_request_body_size,
			max_response_body_size: self.max_response_body_size,
			max_connections: self.max_connections,
			max_subscriptions_per_connection: self.max_subscriptions_per_connection,
			batch_requests_config: self.batch_requests_config,
			tokio_runtime: self.tokio_runtime,
			enable_http: self.enable_http,
			enable_ws: self.enable_ws,
			message_buffer_capacity: self.message_buffer_capacity,
			ping_config: self.ping_config,
			id_provider: self.id_provider,
			tcp_no_delay: self.tcp_no_delay,
			keep_alive: self.keep_alive,
			keep_alive_timeout: self.keep_alive_timeout,
			first_request_timeout: self.first_request_timeout,
		}
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

impl<RpcMiddleware, HttpMiddleware> TowerServiceBuilder<RpcMiddleware, HttpMiddleware> {
	/// Build a tower service.
	pub fn build(
		self,
		methods: impl Into<Methods>,
		stop_handle: StopHandle,
	) -> TowerService<RpcMiddleware, HttpMiddleware> {
		let conn_id = self.conn_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

		let rpc_middleware = TowerServiceNoHttp {
			rpc_middleware: self.rpc_middleware,
			inner: ServiceData {
				methods: methods.into(),
				stop_handle,
				conn_id,
				conn_guard: self.conn_guard,
				server_cfg: self.server_cfg,
			},
			on_session_close: None,
		};

		TowerService { rpc_middleware, http_middleware: self.http_middleware }
	}

	/// Configure the connection id.
	///
	/// This is incremented every time `build` is called.
	pub fn connection_id(mut self, id: u32) -> Self {
		self.conn_id = Arc::new(AtomicU32::new(id));
		self
	}

	/// Configure the max allowed connections on the server.
	pub fn max_connections(mut self, limit: u32) -> Self {
		self.conn_guard = ConnectionGuard::new(limit as usize);
		self
	}

	/// Configure rpc middleware.
	pub fn set_rpc_middleware<T>(self, rpc_middleware: RpcServiceBuilder<T>) -> TowerServiceBuilder<T, HttpMiddleware> {
		TowerServiceBuilder {
			server_cfg: self.server_cfg,
			rpc_middleware,
			http_middleware: self.http_middleware,
			conn_id: self.conn_id,
			conn_guard: self.conn_guard,
		}
	}

	/// Configure http middleware.
	pub fn set_http_middleware<T>(
		self,
		http_middleware: tower::ServiceBuilder<T>,
	) -> TowerServiceBuilder<RpcMiddleware, T> {
		TowerServiceBuilder {
			server_cfg: self.server_cfg,
			rpc_middleware: self.rpc_middleware,
			http_middleware,
			conn_id: self.conn_id,
			conn_guard: self.conn_guard,
		}
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
	///
	/// use std::{time::Instant, net::SocketAddr, sync::Arc};
	/// use std::sync::atomic::{Ordering, AtomicUsize};
	///
	/// use jsonrpsee_server::middleware::rpc::{RpcService, RpcServiceBuilder, RpcServiceT, MethodResponse, Notification, Request, Batch};
	/// use jsonrpsee_server::ServerBuilder;
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
	///     type MethodResponse = S::MethodResponse;
	///     type NotificationResponse = S::NotificationResponse;
	///     type BatchResponse = S::BatchResponse;
	///
	///     fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
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
	///     fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
	///          self.service.batch(batch)
	///     }
	///
	///     fn notification<'a>(&self, notif: Notification<'a>) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
	///          self.service.notification(notif)
	///     }
	/// }
	///
	/// // Create a state per connection
	/// // NOTE: The service type can be omitted once `start` is called on the server.
	/// let m = RpcServiceBuilder::new().layer_fn(move |service: ()| MyMiddleware { service, count: Arc::new(AtomicUsize::new(0)) });
	/// let builder = ServerBuilder::default().set_rpc_middleware(m);
	/// ```
	pub fn set_rpc_middleware<T>(self, rpc_middleware: RpcServiceBuilder<T>) -> Builder<HttpMiddleware, T> {
		Builder { server_cfg: self.server_cfg, rpc_middleware, http_middleware: self.http_middleware }
	}

	/// Configure a custom [`tower::ServiceBuilder`] middleware for composing layers to be applied to the RPC service.
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
	///     let server = jsonrpsee_server::ServerBuilder::new()
	///         .set_http_middleware(builder)
	///         .build("127.0.0.1:0".parse::<SocketAddr>().unwrap())
	///         .await
	///         .unwrap();
	/// }
	/// ```
	pub fn set_http_middleware<T>(self, http_middleware: tower::ServiceBuilder<T>) -> Builder<T, RpcMiddleware> {
		Builder { server_cfg: self.server_cfg, http_middleware, rpc_middleware: self.rpc_middleware }
	}

	/// Convert the server builder to a [`TowerServiceBuilder`].
	///
	/// This can be used to utilize the [`TowerService`] from jsonrpsee.
	///
	/// # Examples
	///
	/// ```no_run
	/// use jsonrpsee_server::{Methods, ServerConfig, ServerHandle, ws, stop_channel, serve_with_graceful_shutdown};
	/// use tower::Service;
	/// use std::{error::Error as StdError, net::SocketAddr};
	/// use futures_util::future::{self, Either};
	/// use hyper_util::rt::{TokioIo, TokioExecutor};
	///
	/// fn run_server() -> ServerHandle {
	///     let (stop_handle, server_handle) = stop_channel();
	///     let svc_builder = jsonrpsee_server::Server::builder()
	///         .set_config(ServerConfig::builder().max_connections(33).build())
	///         .to_service_builder();
	///     let methods = Methods::new();
	///     let stop_handle = stop_handle.clone();
	///
	///     tokio::spawn(async move {
	///         let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
	///
	///         loop {
	///              // The `tokio::select!` macro is used to wait for either of the
	///              // listeners to accept a new connection or for the server to be
	///              // stopped.
	///              let (sock, remote_addr) = tokio::select! {
	///                  res = listener.accept() => {
	///                      match res {
	///                         Ok(sock) => sock,
	///                         Err(e) => {
	///                             tracing::error!("failed to accept v4 connection: {:?}", e);
	///                             continue;
	///                         }
	///                       }
	///                  }
	///                  _ = stop_handle.clone().shutdown() => break,
	///              };
	///
	///              let stop_handle2 = stop_handle.clone();
	///              let svc_builder2 = svc_builder.clone();
	///              let methods2 = methods.clone();
	///
	///              let svc = tower::service_fn(move |req| {
	///                   let stop_handle = stop_handle2.clone();
	///                   let svc_builder = svc_builder2.clone();
	///                   let methods = methods2.clone();
	///
	///                   let mut svc = svc_builder.build(methods, stop_handle.clone());
	///
	///                   // It's not possible to know whether the websocket upgrade handshake failed or not here.
	///                   let is_websocket = ws::is_upgrade_request(&req);
	///
	///                   if is_websocket {
	///                       println!("websocket")
	///                   } else {
	///                       println!("http")
	///                   }
	///
	///                   // Call the jsonrpsee service which
	///                   // may upgrade it to a WebSocket connection
	///                   // or treat it as "ordinary HTTP request".
	///                   async move { svc.call(req).await }
	///               });
	///
	///               // Upgrade the connection to a HTTP service with graceful shutdown.
	///               tokio::spawn(serve_with_graceful_shutdown(sock, svc, stop_handle.clone().shutdown()));
	///          }
	///     });
	///
	///     server_handle
	/// }
	/// ```
	pub fn to_service_builder(self) -> TowerServiceBuilder<RpcMiddleware, HttpMiddleware> {
		let max_conns = self.server_cfg.max_connections as usize;

		TowerServiceBuilder {
			server_cfg: self.server_cfg,
			rpc_middleware: self.rpc_middleware,
			http_middleware: self.http_middleware,
			conn_id: Arc::new(AtomicU32::new(0)),
			conn_guard: ConnectionGuard::new(max_conns),
		}
	}

	/// Finalize the configuration of the server. Consumes the [`Builder`].
	///
	/// ```rust
	/// #[tokio::main]
	/// async fn main() {
	///   let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	///   let occupied_addr = listener.local_addr().unwrap();
	///   let addrs: &[std::net::SocketAddr] = &[
	///       occupied_addr,
	///       "127.0.0.1:0".parse().unwrap(),
	///   ];
	///   assert!(jsonrpsee_server::ServerBuilder::default().build(occupied_addr).await.is_err());
	///   assert!(jsonrpsee_server::ServerBuilder::default().build(addrs).await.is_ok());
	/// }
	/// ```
	///
	pub async fn build(self, addrs: impl ToSocketAddrs) -> std::io::Result<Server<HttpMiddleware, RpcMiddleware>> {
		let listener = TcpListener::bind(addrs).await?;

		Ok(Server {
			listener,
			server_cfg: self.server_cfg,
			rpc_middleware: self.rpc_middleware,
			http_middleware: self.http_middleware,
		})
	}

	/// Finalizes the configuration of the server with customized TCP settings on the socket.
	///
	///
	/// ```rust
	/// use jsonrpsee_server::Server;
	/// use socket2::{Domain, Socket, Type};
	/// use std::time::Duration;
	///
	/// #[tokio::main]
	/// async fn main() {
	///   let addr = "127.0.0.1:0".parse().unwrap();
	///   let domain = Domain::for_address(addr);
	///   let socket = Socket::new(domain, Type::STREAM, None).unwrap();
	///   socket.set_nonblocking(true).unwrap();
	///
	///   let address = addr.into();
	///   socket.bind(&address).unwrap();
	///
	///   socket.listen(4096).unwrap();
	///
	///   let server = Server::builder().build_from_tcp(socket).unwrap();
	/// }
	/// ```
	pub fn build_from_tcp(
		self,
		listener: impl Into<StdTcpListener>,
	) -> std::io::Result<Server<HttpMiddleware, RpcMiddleware>> {
		let listener = TcpListener::from_std(listener.into())?;

		Ok(Server {
			listener,
			server_cfg: self.server_cfg,
			rpc_middleware: self.rpc_middleware,
			http_middleware: self.http_middleware,
		})
	}
}

/// Data required by the server to handle requests.
#[derive(Debug, Clone)]
struct ServiceData {
	/// Registered server methods.
	methods: Methods,
	/// Stop handle.
	stop_handle: StopHandle,
	/// Connection ID
	conn_id: u32,
	/// Connection guard.
	conn_guard: ConnectionGuard,
	/// ServerConfig
	server_cfg: ServerConfig,
}

/// jsonrpsee tower service
///
/// This will enable both `http_middleware` and `rpc_middleware`
/// that may be enabled by [`Builder`] or [`TowerServiceBuilder`].
#[derive(Debug, Clone)]
pub struct TowerService<RpcMiddleware, HttpMiddleware> {
	rpc_middleware: TowerServiceNoHttp<RpcMiddleware>,
	http_middleware: tower::ServiceBuilder<HttpMiddleware>,
}

impl<RpcMiddleware, HttpMiddleware> TowerService<RpcMiddleware, HttpMiddleware> {
	/// A future that returns when the connection has been closed.
	///
	/// This method must be called before every [`TowerService::call`]
	/// because the `SessionClosedFuture` may already been consumed or
	/// not used.
	pub fn on_session_closed(&mut self) -> SessionClosedFuture {
		if let Some(n) = self.rpc_middleware.on_session_close.as_mut() {
			// If it's called more then once another listener is created.
			n.closed()
		} else {
			let (session_close, fut) = session_close();
			self.rpc_middleware.on_session_close = Some(session_close);
			fut
		}
	}
}

impl<RequestBody, ResponseBody, RpcMiddleware, HttpMiddleware> Service<HttpRequest<RequestBody>> for TowerService<RpcMiddleware, HttpMiddleware>
where
	RpcMiddleware: Layer<RpcService> + Clone,
	<RpcMiddleware as Layer<RpcService>>::Service: RpcServiceT + 'static,
	HttpMiddleware: Layer<TowerServiceNoHttp<RpcMiddleware>> + Send + 'static,
	<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service:
		Service<HttpRequest<RequestBody>, Response = HttpResponse<ResponseBody>, Error = BoxError> + Send,
	<<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service as Service<HttpRequest<RequestBody>>>::Future:
		Send + 'static,
	RequestBody: http_body::Body<Data = Bytes> + Send + 'static,
	<RequestBody as http_body::Body>::Error: Into<BoxError>,
{
	type Response = HttpResponse<ResponseBody>;
	type Error = BoxError;
	type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

	fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
		Poll::Ready(Ok(()))
	}

	fn call(&mut self, request: HttpRequest<RequestBody>) -> Self::Future {
		Box::pin(self.http_middleware.service(self.rpc_middleware.clone()).call(request))
	}
}

/// jsonrpsee tower service without HTTP specific middleware.
///
/// # Note
/// This is similar to [`hyper::service::service_fn`].
#[derive(Debug, Clone)]
pub struct TowerServiceNoHttp<L> {
	inner: ServiceData,
	rpc_middleware: RpcServiceBuilder<L>,
	on_session_close: Option<SessionClose>,
}

impl<Body, RpcMiddleware> Service<HttpRequest<Body>> for TowerServiceNoHttp<RpcMiddleware>
where
	RpcMiddleware: Layer<RpcService>,
	<RpcMiddleware as Layer<RpcService>>::Service: RpcServiceT<
			MethodResponse = MethodResponse,
			BatchResponse = MethodResponse,
			NotificationResponse = MethodResponse,
		> + Send
		+ Sync
		+ 'static,
	Body: http_body::Body<Data = Bytes> + Send + 'static,
	<Body as http_body::Body>::Error: Into<BoxError>,
{
	type Response = HttpResponse;

	// The following associated type is required by the `impl<B, U, M: JsonRpcMiddleware> Server<B, L>` bounds.
	// It satisfies the server's bounds when the `tower::ServiceBuilder<B>` is not set (ie `B: Identity`).
	type Error = BoxError;

	type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

	fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
		Poll::Ready(Ok(()))
	}

	fn call(&mut self, request: HttpRequest<Body>) -> Self::Future {
		let mut request = request.map(HttpBody::new);

		let conn_guard = &self.inner.conn_guard;
		let stop_handle = self.inner.stop_handle.clone();
		let conn_id = self.inner.conn_id;
		let on_session_close = self.on_session_close.take();

		tracing::trace!(target: LOG_TARGET, "{:?}", request);

		let Some(conn_permit) = conn_guard.try_acquire() else {
			return async move { Ok(http::response::too_many_requests()) }.boxed();
		};

		let conn = ConnectionState::new(stop_handle.clone(), conn_id, conn_permit);

		let max_conns = conn_guard.max_connections();
		let curr_conns = max_conns - conn_guard.available_connections();
		tracing::debug!(target: LOG_TARGET, "Accepting new connection {}/{}", curr_conns, max_conns);

		let req_ext = request.extensions_mut();
		req_ext.insert::<ConnectionGuard>(conn_guard.clone());
		req_ext.insert::<ConnectionId>(conn.conn_id.into());

		let is_upgrade_request = is_upgrade_request(&request);

		if self.inner.server_cfg.enable_ws && is_upgrade_request {
			let this = self.inner.clone();

			let mut server = soketto::handshake::http::Server::new();

			let response = match server.receive_request(&request) {
				Ok(response) => {
					let (tx, rx) = mpsc::channel(this.server_cfg.message_buffer_capacity as usize);
					let sink = MethodSink::new(tx);

					// On each method call the `pending_calls` is cloned
					// then when all pending_calls are dropped
					// a graceful shutdown can occur.
					let (pending_calls, pending_calls_completed) = mpsc::channel::<()>(1);

					let cfg = RpcServiceCfg::CallsAndSubscriptions {
						bounded_subscriptions: BoundedSubscriptions::new(
							this.server_cfg.max_subscriptions_per_connection,
						),
						id_provider: this.server_cfg.id_provider.clone(),
						sink: sink.clone(),
						_pending_calls: pending_calls,
					};

					let rpc_service = RpcService::new(
						this.methods.clone(),
						this.server_cfg.max_response_body_size as usize,
						this.conn_id.into(),
						cfg,
					);

					let rpc_service = self.rpc_middleware.service(rpc_service);

					tokio::spawn(
						async move {
							let extensions = request.extensions().clone();

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
							ws_builder.set_max_message_size(this.server_cfg.max_request_body_size as usize);
							let (sender, receiver) = ws_builder.finish();

							let params = BackgroundTaskParams {
								server_cfg: this.server_cfg,
								conn,
								ws_sender: sender,
								ws_receiver: receiver,
								rpc_service,
								sink,
								rx,
								pending_calls_completed,
								on_session_close,
								extensions,
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
			let methods = this.methods.clone();
			let batch_config = this.server_cfg.batch_requests_config;

			let rpc_service = self.rpc_middleware.service(RpcService::new(
				methods,
				max_response_size as usize,
				this.conn_id.into(),
				RpcServiceCfg::OnlyCalls,
			));

			Box::pin(async move {
				let rp = http::call_with_service(request, batch_config, max_request_size, rpc_service).await;
				// NOTE: The `conn guard` must be held until the response is processed
				// to respect the `max_connections` limit.
				drop(conn);
				Ok(rp)
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
	conn_id: u32,
	server_cfg: ServerConfig,
	stop_handle: StopHandle,
	socket: TcpStream,
	drop_on_completion: mpsc::Sender<()>,
	remote_addr: SocketAddr,
	methods: Methods,
}

#[instrument(name = "connection", skip_all, fields(remote_addr = %params.remote_addr, conn_id = %params.conn_id), level = "INFO")]
fn process_connection<'a, RpcMiddleware, HttpMiddleware, Body>(params: ProcessConnection<HttpMiddleware, RpcMiddleware>)
where
	HttpMiddleware: Layer<TowerServiceNoHttp<RpcMiddleware>> + Send + 'static,
	<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service:
		Service<HttpRequest, Response = HttpResponse<Body>, Error = BoxError> + Clone + Send + 'static,
	<<HttpMiddleware as Layer<TowerServiceNoHttp<RpcMiddleware>>>::Service as Service<HttpRequest>>::Future:
		Send + 'static,
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

	if let Err(e) = socket.set_nodelay(server_cfg.tcp_no_delay) {
		tracing::warn!(target: LOG_TARGET, "Could not set NODELAY on socket: {:?}", e);
		return;
	}

	let keep_alive = server_cfg.keep_alive;
	let keep_alive_timeout = server_cfg.keep_alive_timeout;
	let first_request_timeout = server_cfg.first_request_timeout;

	let tower_service = TowerServiceNoHttp {
		inner: ServiceData {
			server_cfg,
			methods,
			stop_handle: stop_handle.clone(),
			conn_id,
			conn_guard: conn_guard.clone(),
		},
		rpc_middleware,
		on_session_close: None,
	};

	let service = http_middleware.service(tower_service);

	tokio::spawn(async move {
		let request_received = Arc::new(Notify::new());
		// this requires Clone.
		let service = NotifyOnRequest::new(crate::utils::TowerToHyperService::new(service), request_received.clone());
		let io = TokioIo::new(socket);
		let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());

		//default is true for http1, if set to false then websocket connections will not be upgraded.
		builder.http2().keep_alive_interval(keep_alive).keep_alive_timeout(keep_alive_timeout);

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
	});
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

	match futures_util::future::select(accept, stopped).await {
		Either::Left((res, stop)) => match res {
			Ok((socket, remote_addr)) => AcceptConnection::Established { socket, remote_addr, stop },
			Err(e) => AcceptConnection::Err((e, stop)),
		},
		Either::Right(_) => AcceptConnection::Shutdown,
	}
}

pub(crate) async fn handle_rpc_call<S>(
	body: &[u8],
	is_single: bool,
	batch_config: BatchRequestConfig,
	rpc_service: &S,
	extensions: Extensions,
) -> MethodResponse
where
	S: RpcServiceT<
			MethodResponse = MethodResponse,
			BatchResponse = MethodResponse,
			NotificationResponse = MethodResponse,
		> + Send,
{
	// Single request or notification
	if is_single {
		if let Ok(req) = deserialize_with_ext::call::from_slice(body, &extensions) {
			rpc_service.call(req).await
		} else if let Ok(notif) = deserialize_with_ext::notif::from_slice::<Notif>(body, &extensions) {
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
					ErrorObject::borrowed(BATCHES_NOT_SUPPORTED_CODE, BATCHES_NOT_SUPPORTED_MSG, None),
				);
				return rp;
			}
			BatchRequestConfig::Limit(limit) => limit as usize,
			BatchRequestConfig::Unlimited => usize::MAX,
		};

		if let Ok(unchecked_batch) = serde_json::from_slice::<Vec<&JsonRawValue>>(body) {
			if unchecked_batch.len() > max_len {
				return MethodResponse::error(Id::Null, reject_too_big_batch_request(max_len));
			}

			let mut batch = Vec::with_capacity(unchecked_batch.len());

			for call in unchecked_batch {
				if let Ok(req) = deserialize_with_ext::call::from_str(call.get(), &extensions) {
					batch.push(Ok(BatchEntry::Call(req)));
				} else if let Ok(notif) = deserialize_with_ext::notif::from_str::<Notif>(call.get(), &extensions) {
					batch.push(Ok(BatchEntry::Notification(notif)));
				} else {
					let id = match serde_json::from_str::<jsonrpsee_types::InvalidRequest>(call.get()) {
						Ok(err) => err.id,
						Err(_) => Id::Null,
					};

					batch.push(Err(BatchEntryErr::new(id, ErrorCode::InvalidRequest.into())));
				}
			}

			rpc_service.batch(Batch::from(batch)).await
		} else {
			MethodResponse::error(Id::Null, ErrorObject::from(ErrorCode::ParseError))
		}
	}
}
