use crate::tests::helpers::{init_logger, server_with_handles};
use crate::{HttpBody, HttpRequest, HttpResponse, ServerConfig};
use hyper::StatusCode;
use jsonrpsee_core::BoxError;
use jsonrpsee_test_utils::TimeoutFutureExt;
use jsonrpsee_test_utils::helpers::{http_request, ok_response, to_http_uri};
use jsonrpsee_test_utils::mocks::{Id, WebSocketTestClient, WebSocketTestError};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn stop_works() {
	init_logger();
	let (_addr, server_handle) = server_with_handles().with_default_timeout().await.unwrap();

	let handle = server_handle.clone();
	handle.stop().unwrap();
	handle.stopped().await;

	// After that we should be able to wait for task handle to finish.
	// First `unwrap` is timeout, second is `JoinHandle`'s one.

	// After server was stopped, attempt to stop it again should result in an error.
	assert!(server_handle.stop().is_err());
}

#[tokio::test]
async fn run_forever() {
	const TIMEOUT: Duration = Duration::from_millis(200);

	init_logger();
	let (_addr, server_handle) = server_with_handles().with_default_timeout().await.unwrap();

	assert!(matches!(server_handle.stopped().with_timeout(TIMEOUT).await, Err(_timeout_err)));

	let (_addr, server_handle) = server_with_handles().with_default_timeout().await.unwrap();

	server_handle.stop().unwrap();

	// Send the shutdown request from one handle and await the server on the second one.
	server_handle.stopped().with_timeout(TIMEOUT).await.unwrap();
}

#[tokio::test]
async fn http_only_works() {
	use crate::{RpcModule, ServerBuilder};

	let config = ServerConfig::builder().http_only().build();
	let server = ServerBuilder::with_config(config).build("127.0.0.1:0").with_default_timeout().await.unwrap().unwrap();
	let mut module = RpcModule::new(());
	module
		.register_method("say_hello", |_, _, _| {
			tracing::debug!("server respond to hello");
			"hello"
		})
		.unwrap();

	let addr = server.local_addr().unwrap();
	let _server_handle = server.start(module);

	let req = r#"{"jsonrpc":"2.0","method":"say_hello","id":1}"#;
	let response = http_request(req.into(), to_http_uri(addr)).with_default_timeout().await.unwrap().unwrap();
	assert_eq!(response.status, StatusCode::OK);
	assert_eq!(response.body, ok_response("hello".to_string().into(), Id::Num(1)));

	let err = WebSocketTestClient::new(addr).with_default_timeout().await.unwrap().unwrap_err();
	assert!(matches!(err, WebSocketTestError::RejectedWithStatusCode(code) if code == 403));
}

#[tokio::test]
async fn ws_only_works() {
	use crate::{RpcModule, ServerBuilder};

	let config = ServerConfig::builder().ws_only().build();
	let server = ServerBuilder::with_config(config).build("127.0.0.1:0").with_default_timeout().await.unwrap().unwrap();
	let mut module = RpcModule::new(());
	module
		.register_method("say_hello", |_, _, _| {
			tracing::debug!("server respond to hello");
			"hello"
		})
		.unwrap();

	let addr = server.local_addr().unwrap();
	let _server_handle = server.start(module);

	let req = r#"{"jsonrpc":"2.0","method":"say_hello","id":1}"#;
	let response = http_request(req.into(), to_http_uri(addr)).with_default_timeout().await.unwrap().unwrap();
	assert_eq!(response.status, StatusCode::FORBIDDEN);

	let mut client = WebSocketTestClient::new(addr).with_default_timeout().await.unwrap().unwrap();
	let response = client.send_request_text(req.to_string()).await.unwrap();
	assert_eq!(response, ok_response("hello".to_string().into(), Id::Num(1)));
}

async fn server_with_first_request_timeout(first_request_timeout: Duration) -> SocketAddr {
	use crate::{RpcModule, ServerBuilder};

	let config = ServerConfig::builder().set_first_request_timeout(Some(first_request_timeout)).build();
	let server = ServerBuilder::with_config(config).build("127.0.0.1:0").with_default_timeout().await.unwrap().unwrap();
	let mut module = RpcModule::new(());
	module.register_method("say_hello", |_, _, _| "hello").unwrap();
	module
		.register_async_method("slow_hello", move |_, _, _| async move {
			tokio::time::sleep(first_request_timeout * 3).await;
			"hello"
		})
		.unwrap();

	let addr = server.local_addr().unwrap();
	tokio::spawn(server.start(module).stopped());
	addr
}

#[tokio::test]
async fn first_request_timeout_closes_connection_that_sends_nothing() {
	let addr = server_with_first_request_timeout(Duration::from_millis(100)).await;
	let mut stream = TcpStream::connect(addr).with_default_timeout().await.unwrap().unwrap();

	let closed = stream.read_to_end(&mut Vec::new()).with_timeout(Duration::from_secs(5)).await;
	assert!(closed.is_ok(), "connection that never sent a request was kept open");
}

#[tokio::test]
async fn first_request_timeout_closes_connection_with_incomplete_header() {
	let addr = server_with_first_request_timeout(Duration::from_millis(100)).await;
	let mut stream = TcpStream::connect(addr).with_default_timeout().await.unwrap().unwrap();
	stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

	let closed = stream.read_to_end(&mut Vec::new()).with_timeout(Duration::from_secs(5)).await;
	assert!(closed.is_ok(), "connection whose request header never completed was kept open");
}

#[tokio::test]
async fn first_request_timeout_closes_connection_with_incomplete_http2_preface() {
	let addr = server_with_first_request_timeout(Duration::from_millis(100)).await;
	let mut stream = TcpStream::connect(addr).with_default_timeout().await.unwrap().unwrap();
	stream.write_all(b"P").await.unwrap();

	let closed = stream.read_to_end(&mut Vec::new()).with_timeout(Duration::from_secs(5)).await;
	assert!(closed.is_ok(), "connection whose HTTP/2 preface never completed was kept open");
}

#[tokio::test]
async fn first_request_timeout_closes_http2_connection_without_request() {
	let addr = server_with_first_request_timeout(Duration::from_millis(100)).await;
	let mut stream = TcpStream::connect(addr).with_default_timeout().await.unwrap().unwrap();
	stream.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await.unwrap();

	let closed = stream.read_to_end(&mut Vec::new()).with_timeout(Duration::from_secs(5)).await;
	assert!(closed.is_ok(), "HTTP/2 connection without a request was kept open");
}

#[tokio::test]
async fn first_request_timeout_does_not_cut_slow_request() {
	let addr = server_with_first_request_timeout(Duration::from_millis(100)).await;

	let req = r#"{"jsonrpc":"2.0","method":"slow_hello","id":1}"#;
	let response = http_request(req.into(), to_http_uri(addr)).with_default_timeout().await.unwrap().unwrap();
	assert_eq!(response.status, StatusCode::OK);
	assert_eq!(response.body, ok_response("hello".to_string().into(), Id::Num(1)));
}

#[tokio::test]
async fn first_request_timeout_counts_request_waiting_for_middleware() {
	use crate::{RpcModule, ServerBuilder};
	use std::sync::Arc;
	use tokio::sync::Notify;

	// Only one request is processed at a time, a second one waits in `poll_ready` of the middleware.
	let middleware = tower::ServiceBuilder::new().layer(tower::limit::GlobalConcurrencyLimitLayer::new(1));
	let config = ServerConfig::builder().set_first_request_timeout(Some(Duration::from_millis(100))).build();
	let server = ServerBuilder::with_config(config)
		.set_http_middleware(middleware)
		.build("127.0.0.1:0")
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();
	let slow_started = Arc::new(Notify::new());
	let mut module = RpcModule::new(slow_started.clone());
	module.register_method("say_hello", |_, _, _| "hello").unwrap();
	module
		.register_async_method("slow_hello", |_, slow_started, _| async move {
			slow_started.notify_one();
			tokio::time::sleep(Duration::from_millis(300)).await;
			"hello"
		})
		.unwrap();
	let uri = to_http_uri(server.local_addr().unwrap());
	tokio::spawn(server.start(module).stopped());

	let slow = tokio::spawn(http_request(r#"{"jsonrpc":"2.0","method":"slow_hello","id":1}"#.into(), uri.clone()));
	slow_started.notified().await;

	let req = r#"{"jsonrpc":"2.0","method":"say_hello","id":1}"#;
	let response = http_request(req.into(), uri).with_default_timeout().await.unwrap();
	assert!(response.is_ok(), "request waiting for the middleware was dropped: {:?}", response.err());
	assert!(slow.await.unwrap().is_ok());
}

#[tokio::test]
async fn first_request_timeout_keeps_idle_websocket_open() {
	let addr = server_with_first_request_timeout(Duration::from_millis(100)).await;
	let mut client = WebSocketTestClient::new(addr).with_default_timeout().await.unwrap().unwrap();
	tokio::time::sleep(Duration::from_millis(300)).await;

	let req = r#"{"jsonrpc":"2.0","method":"say_hello","id":1}"#;
	let response = client.send_request_text(req).with_default_timeout().await.unwrap().unwrap();
	assert_eq!(response, ok_response("hello".to_string().into(), Id::Num(1)));
}

#[tokio::test(start_paused = true)]
async fn serve_closes_connection_that_sends_nothing() {
	let (_client, io) = tokio::io::duplex(1024);
	let service = tower::service_fn(|_: HttpRequest<hyper::body::Incoming>| async {
		Ok::<_, BoxError>(HttpResponse::new(HttpBody::empty()))
	});

	let served = crate::serve_with_graceful_shutdown(io, service, std::future::pending::<()>())
		.with_timeout(Duration::from_secs(60))
		.await;
	assert!(served.is_ok(), "connection that never sent a request was kept open");
}

#[tokio::test(start_paused = true)]
async fn serve_closes_connection_with_incomplete_header() {
	let (mut client, io) = tokio::io::duplex(1024);
	client.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
	let service = tower::service_fn(|_: HttpRequest<hyper::body::Incoming>| async {
		Ok::<_, BoxError>(HttpResponse::new(HttpBody::empty()))
	});

	let served = crate::serve_with_graceful_shutdown(io, service, std::future::pending::<()>())
		.with_timeout(Duration::from_secs(60))
		.await;
	assert!(served.is_ok(), "connection whose request header never completed was kept open");
}
