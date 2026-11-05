use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::{fmt, sync::atomic::AtomicUsize};

use crate::{
	RpcModule, Server, ServerBuilder, ServerConfigBuilder, ServerHandle, serve_with_graceful_shutdown, stop_channel,
};

use futures_util::FutureExt;
use jsonrpsee_core::server::Methods;
use jsonrpsee_core::{DeserializeOwned, RpcResult, SubscriptionError};
use jsonrpsee_test_utils::TimeoutFutureExt;
use jsonrpsee_types::{ErrorObject, ErrorObjectOwned, Response, ResponseSuccess, error::ErrorCode};
use tokio::net::TcpListener;
use tower::Service;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

pub(crate) struct TestContext;

pub(crate) async fn server_with_call_counter() -> (SocketAddr, ServerHandle, Arc<AtomicUsize>) {
	let server = ServerBuilder::default().build("127.0.0.1:0").with_default_timeout().await.unwrap().unwrap();
	let calls = Arc::new(AtomicUsize::new(0));
	let mut module = RpcModule::new(calls.clone());
	module.register_method("tick", |_, calls, _| calls.fetch_add(1, Ordering::SeqCst)).unwrap();
	let addr = server.local_addr().unwrap();
	(addr, server.start(module), calls)
}

impl TestContext {
	pub(crate) fn ok(&self) -> Result<(), MyAppError> {
		Ok(())
	}
	pub(crate) fn err(&self) -> Result<(), MyAppError> {
		Err(MyAppError)
	}
}

/// Spawns a dummy JSON-RPC server.
pub(crate) async fn server() -> SocketAddr {
	let (addr, handle) = server_with_handles().await;
	tokio::spawn(handle.stopped());
	addr
}

/// Spawns a dummy JSON-RPC server.
///
/// Returns the address together with handle for the server.
pub(crate) async fn server_with_handles() -> (SocketAddr, ServerHandle) {
	let server = ServerBuilder::default().build("127.0.0.1:0").with_default_timeout().await.unwrap().unwrap();
	let ctx = TestContext;
	let mut module = RpcModule::new(ctx);
	module
		.register_method("say_hello", |_, _, _| {
			tracing::debug!("server respond to hello");
			"hello"
		})
		.unwrap();
	module
		.register_method::<Result<u64, ErrorObjectOwned>, _>("add", |params, _, _| {
			let params: Vec<u64> = params.parse()?;
			let sum: u64 = params.into_iter().sum();
			Ok(sum)
		})
		.unwrap();
	module
		.register_method::<Result<String, ErrorObjectOwned>, _>("multiparam", |params, _, _| {
			let params: (String, String, Vec<u8>) = params.parse()?;
			let r = format!("string1={}, string2={}, vec={}", params.0.len(), params.1.len(), params.2.len());
			Ok(r)
		})
		.unwrap();
	module
		.register_async_method("say_hello_async", |_, _, _| {
			async move {
				tracing::debug!("server respond to hello");
				// Call some async function inside.
				futures_util::future::ready(()).await;
				"hello"
			}
		})
		.unwrap();
	module
		.register_async_method::<Result<u64, ErrorObjectOwned>, _, _>("add_async", |params, _, _| async move {
			let params: Vec<u64> = params.parse()?;
			let sum: u64 = params.into_iter().sum();
			Ok(sum)
		})
		.unwrap();
	module.register_method("invalid_params", |_params, _, _| Err::<(), _>(invalid_params())).unwrap();
	module.register_method("call_fail", |_params, _, _| Err::<(), _>(MyAppError)).unwrap();
	module
		.register_method::<Result<&str, ErrorObjectOwned>, _>("sleep_for", |params, _, _| {
			let sleep: Vec<u64> = params.parse()?;
			std::thread::sleep(std::time::Duration::from_millis(sleep[0]));
			Ok("Yawn!")
		})
		.unwrap();
	module
		.register_subscription::<Result<(), SubscriptionError>, _, _>(
			"subscribe_hello",
			"subscribe_hello",
			"unsubscribe_hello",
			|_, pending, _, _| async move {
				let sink = pending.accept().await?;

				loop {
					let _ = &sink;
					tokio::time::sleep(std::time::Duration::from_secs(30)).await;
				}
			},
		)
		.unwrap();

	module.register_method("notif", |_, _, _| "").unwrap();
	module
		.register_method("should_err", |_, ctx, _| {
			ctx.err()?;
			RpcResult::Ok("err")
		})
		.unwrap();

	module
		.register_method("should_ok", |_, ctx, _| {
			ctx.ok()?;
			RpcResult::Ok("ok")
		})
		.unwrap();
	module
		.register_async_method("should_ok_async", |_p, ctx, _| async move {
			ctx.ok()?;
			Ok::<_, MyAppError>("ok")
		})
		.unwrap();

	let addr = server.local_addr().unwrap();

	let server_handle = server.start(module);
	(addr, server_handle)
}

/// Run server with user provided context.
pub(crate) async fn server_with_context() -> SocketAddr {
	let server = ServerBuilder::default().build("127.0.0.1:0").with_default_timeout().await.unwrap().unwrap();

	let ctx = TestContext;
	let mut rpc_module = RpcModule::new(ctx);

	rpc_module
		.register_method("should_err", |_p, ctx, _| {
			ctx.err()?;
			RpcResult::Ok("err")
		})
		.unwrap();

	rpc_module
		.register_method("should_ok", |_p, ctx, _| {
			ctx.ok()?;
			RpcResult::Ok("ok")
		})
		.unwrap();

	rpc_module
		.register_async_method("should_ok_async", |_p, ctx, _| async move {
			ctx.ok()?;
			// Call some async function inside.
			Result::<_, MyAppError>::Ok(futures_util::future::ready("ok!").await)
		})
		.unwrap();

	rpc_module
		.register_async_method("err_async", |_p, ctx, _| async move {
			ctx.ok()?;
			// Async work that returns an error
			futures_util::future::err::<(), _>(MyAppError).await
		})
		.unwrap();

	let addr = server.local_addr().unwrap();
	let handle = server.start(rpc_module);

	tokio::spawn(handle.stopped());
	addr
}

pub(crate) fn init_logger() {
	let _ = FmtSubscriber::builder().with_env_filter(EnvFilter::from_default_env()).try_init();
}

pub(crate) fn deser_call<T: DeserializeOwned + fmt::Debug + Clone>(raw: String) -> T {
	let rp: Response<T> = serde_json::from_str(&raw).unwrap();
	ResponseSuccess::try_from(rp).expect("Successful call").result
}

/// Applications can/should provide their own error.
#[derive(Copy, Clone, Debug)]
pub struct MyAppError;
impl fmt::Display for MyAppError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "MyAppError")
	}
}
impl std::error::Error for MyAppError {}

impl From<MyAppError> for ErrorObjectOwned {
	fn from(_: MyAppError) -> Self {
		ErrorObject::owned(-32000, "MyAppError", None::<()>)
	}
}

fn invalid_params() -> ErrorObjectOwned {
	ErrorCode::InvalidParams.into()
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Metrics {
	pub(crate) ws_sessions_opened: Arc<AtomicUsize>,
	pub(crate) ws_sessions_closed: Arc<AtomicUsize>,
}

pub(crate) async fn ws_server_with_stats(metrics: Metrics) -> SocketAddr {
	let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
	let addr = listener.local_addr().unwrap();
	let (stop_handle, server_handle) = stop_channel();
	let metrics = metrics.clone();

	let rpc_svc = Server::builder()
		.set_config(ServerConfigBuilder::new().max_connections(33).build())
		.to_service_builder()
		.build(Methods::new(), stop_handle.clone());

	tokio::spawn(async move {
		loop {
			let sock = tokio::select! {
				res = listener.accept() => {
					match res {
						Ok((stream, _remote_addr)) => stream,
						Err(e) => {
							tracing::error!("failed to accept v4 connection: {:?}", e);
							continue;
						}
					}
				}
				_ = stop_handle.clone().shutdown() => break,
			};

			let rpc_svc = rpc_svc.clone();
			let metrics = metrics.clone();
			let stop_handle = stop_handle.clone();

			tokio::spawn(async move {
				let rpc_svc = rpc_svc.clone();

				let svc = tower::service_fn(move |req| {
					let is_websocket = crate::ws::is_upgrade_request(&req);
					let metrics = metrics.clone();
					let mut rpc_svc = rpc_svc.clone();

					if is_websocket {
						// This should work for each callback.
						let session_close1 = rpc_svc.on_session_closed();
						let session_close2 = rpc_svc.on_session_closed();

						tokio::spawn(async move {
							metrics.ws_sessions_opened.fetch_add(1, Ordering::SeqCst);
							tokio::join!(session_close2, session_close1);
							metrics.ws_sessions_closed.fetch_add(1, Ordering::SeqCst);
						});
					}

					async move { rpc_svc.call(req).await }.boxed()
				});

				tokio::spawn(serve_with_graceful_shutdown(sock, svc, stop_handle.clone().shutdown()));
			});
		}
	});

	tokio::spawn(server_handle.stopped());

	addr
}
