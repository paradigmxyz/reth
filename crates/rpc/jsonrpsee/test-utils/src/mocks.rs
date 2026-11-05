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

use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use futures_channel::mpsc;
use futures_channel::oneshot;
use futures_util::future::FutureExt;
use futures_util::io::{BufReader, BufWriter};
use futures_util::sink::SinkExt;
use futures_util::stream::{self, StreamExt};
use futures_util::{pin_mut, select};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::{Deserialize, Serialize};
use soketto::handshake::{self, Error as SokettoError, Server, http::is_upgrade_request, server::Response};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

pub use hyper::{HeaderMap, StatusCode, Uri};

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
type EmptyBody = http_body_util::Empty<hyper::body::Bytes>;

/// Request Id
#[derive(Debug, PartialEq, Clone, Hash, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum Id {
	/// No id (notification)
	Null,
	/// Numeric id
	Num(u64),
	/// String id
	Str(String),
}

#[derive(Debug)]
pub struct HttpResponse {
	pub status: StatusCode,
	pub header: HeaderMap,
	pub body: String,
}

/// WebSocket client to construct with arbitrary payload to construct bad payloads.
pub struct WebSocketTestClient {
	tx: soketto::Sender<BufReader<BufWriter<Compat<TcpStream>>>>,
	rx: soketto::Receiver<BufReader<BufWriter<Compat<TcpStream>>>>,
}

impl std::fmt::Debug for WebSocketTestClient {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "WebSocketTestClient")
	}
}

#[derive(Debug)]
pub enum WebSocketTestError {
	Redirect,
	RejectedWithStatusCode(u16),
	Soketto(SokettoError),
}

impl From<io::Error> for WebSocketTestError {
	fn from(err: io::Error) -> Self {
		WebSocketTestError::Soketto(SokettoError::Io(err))
	}
}

impl WebSocketTestClient {
	pub async fn new(url: SocketAddr) -> Result<Self, WebSocketTestError> {
		let socket = TcpStream::connect(url).await?;
		let mut client = handshake::Client::new(BufReader::new(BufWriter::new(socket.compat())), "test-client", "/");
		match client.handshake().await {
			Ok(handshake::ServerResponse::Accepted { .. }) => {
				let (tx, rx) = client.into_builder().finish();
				Ok(Self { tx, rx })
			}
			Ok(handshake::ServerResponse::Redirect { .. }) => Err(WebSocketTestError::Redirect),
			Ok(handshake::ServerResponse::Rejected { status_code }) => {
				Err(WebSocketTestError::RejectedWithStatusCode(status_code))
			}
			Err(err) => Err(WebSocketTestError::Soketto(err)),
		}
	}

	pub async fn send_request_text(&mut self, msg: impl AsRef<str>) -> Result<String, Error> {
		self.send(msg).await?;
		let mut data = Vec::new();
		self.rx.receive_data(&mut data).await?;
		String::from_utf8(data).map_err(Into::into)
	}

	pub async fn send(&mut self, msg: impl AsRef<str>) -> Result<(), Error> {
		self.tx.send_text(msg).await?;
		self.tx.flush().await.map_err(Into::into)
	}

	pub async fn send_request_binary(&mut self, msg: &[u8]) -> Result<String, Error> {
		self.tx.send_binary(msg).await?;
		self.tx.flush().await?;
		self.receive().await
	}

	pub async fn receive(&mut self) -> Result<String, Error> {
		let mut data = Vec::new();
		self.rx.receive_data(&mut data).await?;
		String::from_utf8(data).map_err(Into::into)
	}

	pub async fn close(&mut self) -> Result<(), Error> {
		self.tx.close().await.map_err(Into::into)
	}
}

#[derive(Debug, Clone)]
pub enum ServerMode {
	// Send out a hardcoded response on every connection.
	Response(String),
	// Send out a subscription ID on a request and continuously send out data on the subscription.
	Subscription { subscription_id: String, subscription_response: String },
	// Send out a notification after timeout
	Notification(String),
}

/// JSONRPC v2 dummy WebSocket server that sends a hardcoded response.
pub struct WebSocketTestServer {
	local_addr: SocketAddr,
	exit: mpsc::UnboundedSender<()>,
}

impl WebSocketTestServer {
	// Spawns a dummy `JSONRPC v2` WebSocket server that sends out a pre-configured `hardcoded response` for every
	// connection.
	pub async fn with_hardcoded_response(sockaddr: SocketAddr, response: String) -> Self {
		let listener = tokio::net::TcpListener::bind(sockaddr).await.unwrap();
		let local_addr = listener.local_addr().unwrap();
		let (tx, rx) = mpsc::unbounded();
		tokio::spawn(server_backend(listener, rx, ServerMode::Response(response)));

		Self { local_addr, exit: tx }
	}

	// Spawns a dummy `JSONRPC v2` WebSocket server that sends out a pre-configured `hardcoded notification` for every
	// connection.
	pub async fn with_hardcoded_notification(sockaddr: SocketAddr, notification: String) -> Self {
		let (tx, rx) = mpsc::unbounded();
		let (addr_tx, addr_rx) = oneshot::channel();

		tokio::spawn(async move {
			let listener = tokio::net::TcpListener::bind(sockaddr).await.unwrap();
			let local_addr = listener.local_addr().unwrap();

			addr_tx.send(local_addr).unwrap();
			server_backend(listener, rx, ServerMode::Notification(notification)).await
		});

		let local_addr = addr_rx.await.unwrap();

		Self { local_addr, exit: tx }
	}

	// Spawns a dummy `JSONRPC v2` WebSocket server that sends out a pre-configured subscription ID and subscription
	// response.
	//
	// NOTE: ignores the actual subscription and unsubscription method.
	pub async fn with_hardcoded_subscription(
		sockaddr: SocketAddr,
		subscription_id: String,
		subscription_response: String,
	) -> Self {
		let listener = tokio::net::TcpListener::bind(sockaddr).await.unwrap();
		let local_addr = listener.local_addr().unwrap();
		let (tx, rx) = mpsc::unbounded();
		tokio::spawn(server_backend(listener, rx, ServerMode::Subscription { subscription_id, subscription_response }));

		Self { local_addr, exit: tx }
	}

	pub fn local_addr(&self) -> SocketAddr {
		self.local_addr
	}

	pub async fn close(&mut self) {
		self.exit.send(()).await.unwrap();
	}
}

async fn server_backend(listener: tokio::net::TcpListener, mut exit: mpsc::UnboundedReceiver<()>, mode: ServerMode) {
	let mut connections = Vec::new();

	loop {
		let conn_fut = listener.accept().fuse();
		let exit_fut = exit.next();
		pin_mut!(exit_fut, conn_fut);

		select! {
			_ = exit_fut => break,
			conn = conn_fut => {
				if let Ok((stream, _)) = conn {
					let (tx, rx) = mpsc::unbounded();
					let handle = tokio::spawn(connection_task(stream, mode.clone(), rx));
					connections.push((handle, tx));
				}
			}
		}
	}

	// close connections
	for (handle, mut exit) in connections {
		// If the actual connection was never established i.e., returned early
		// It will most likely be caught on the client-side but just to be explicit.
		exit.send(()).await.expect("WebSocket connection was never established");
		handle.await.unwrap();
	}
}

async fn connection_task(socket: tokio::net::TcpStream, mode: ServerMode, mut exit: mpsc::UnboundedReceiver<()>) {
	let mut server = Server::new(socket.compat());

	let key = match server.receive_request().await {
		Ok(req) => req.key(),
		Err(_) => return,
	};

	let accept = server.send_response(&Response::Accept { key, protocol: None }).await;

	if accept.is_err() {
		return;
	}

	let (mut sender, receiver) = server.into_builder().finish();

	let ws_stream = stream::unfold(receiver, move |mut receiver| async {
		let mut buf = Vec::new();
		let ret = match receiver.receive_data(&mut buf).await {
			Ok(_) => Ok(buf),
			Err(err) => Err(err),
		};
		Some((ret, receiver))
	});
	pin_mut!(ws_stream);

	loop {
		let next_ws = ws_stream.next().fuse();
		let next_exit = exit.next().fuse();
		let time_out = tokio::time::sleep(Duration::from_millis(200)).fuse();

		pin_mut!(time_out, next_exit, next_ws);

		select! {
			_ = time_out => {
				 match &mode {
					ServerMode::Subscription { subscription_response, .. } => {
						if let Err(e) = sender.send_text(&subscription_response).await {
							tracing::warn!("send response to subscription: {:?}", e);
							break;
						}
					},
					ServerMode::Notification(n) => {
						if let Err(e) = sender.send_text(&n).await {
							tracing::warn!("send notification: {:?}", e);
							break;
						}
					},
					_ => {}
				}
			}
			ws = next_ws => {
				// Got a request on the connection but don't care about the contents.
				// Just send out the pre-configured hardcoded responses.
				if let Some(Ok(_)) = ws {
					match &mode {
						ServerMode::Response(r) => {
							if let Err(e) = sender.send_text(&r).await {
								tracing::warn!("send response to request error: {:?}", e);
								break;
							}
						},
						ServerMode::Subscription { subscription_id, .. } => {
							if let Err(e) = sender.send_text(&subscription_id).await {
								tracing::warn!("send subscription id error: {:?}", e);
								break;
							}
						},
						_ => {}
					}
				} else {
					break;
				}
			}
			_ = next_exit => break,
		}
	}
}

// Run a WebSocket server running on localhost that redirects requests for testing.
// Requests to any url except for `/myblock/two` will redirect one or two times (HTTP 301) and eventually end up in `/myblock/two`.
pub async fn ws_server_with_redirect(other_server: String) -> String {
	let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
	let addr = listener.local_addr().unwrap();

	tokio::spawn(async move {
		loop {
			let Ok((stream, _addr)) = listener.accept().await else {
				continue;
			};

			let other_server = other_server.clone();
			tokio::spawn(async {
				let io = TokioIo::new(stream);
				let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());

				let conn = builder.serve_connection_with_upgrades(
					io,
					hyper::service::service_fn(move |req| {
						let other_server = other_server.clone();
						handler(req, other_server)
					}),
				);

				conn.await.unwrap();
			});
		}
	});
	format!("ws://{addr}")
}

/// Handle incoming HTTP Requests.
async fn handler(
	req: hyper::Request<hyper::body::Incoming>,
	other_server: String,
) -> Result<hyper::Response<EmptyBody>, Infallible> {
	if is_upgrade_request(&req) {
		tracing::debug!("{:?}", req);

		match req.uri().path() {
			"/myblock/two" => {
				let response = hyper::Response::builder()
					.status(301)
					.header("Location", other_server)
					.body(EmptyBody::new())
					.unwrap();
				Ok(response)
			}
			"/myblock/one" => {
				let response =
					hyper::Response::builder().status(301).header("Location", "two").body(EmptyBody::new()).unwrap();
				Ok(response)
			}
			_ => {
				let response = hyper::Response::builder()
					.status(301)
					.header("Location", "/myblock/one")
					.body(EmptyBody::new())
					.unwrap();
				Ok(response)
			}
		}
	} else {
		panic!("expect upgrade to WS");
	}
}
