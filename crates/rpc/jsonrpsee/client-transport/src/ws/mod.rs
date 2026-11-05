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

mod stream;

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine;
use futures_util::io::{BufReader, BufWriter};
use jsonrpsee_core::Cow;
use jsonrpsee_core::TEN_MB_SIZE_BYTES;
use jsonrpsee_core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};
use soketto::connection::CloseReason;
use soketto::connection::Error::Utf8;
use soketto::data::ByteSlice125;
use soketto::handshake::client::{Client as WsHandshakeClient, ServerResponse};
use soketto::{Data, Incoming, connection};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

pub use http::{HeaderMap, HeaderValue, Uri, uri::InvalidUri};
pub use soketto::handshake::client::Header;
pub use stream::EitherStream;
pub use tokio::io::{AsyncRead, AsyncWrite};
pub use url::Url;

const LOG_TARGET: &str = "jsonrpsee-client";

/// Custom TLS configuration.
#[cfg(feature = "tls")]
pub type CustomCertStore = rustls::ClientConfig;

/// Certificate store to use for TLS connections.
// rustls needs the concrete `ClientConfig` type so we can't Box it here.
#[allow(clippy::large_enum_variant)]
#[cfg(feature = "tls")]
#[derive(Debug, Clone)]
pub enum CertificateStore {
	/// Native.
	Native,
	/// Custom certificate store.
	Custom(CustomCertStore),
}

/// Sending end of WebSocket transport.
#[derive(Debug)]
pub struct Sender<T> {
	inner: connection::Sender<BufReader<BufWriter<T>>>,
	max_request_size: u32,
}

/// Receiving end of WebSocket transport.
#[derive(Debug)]
pub struct Receiver<T> {
	inner: connection::Receiver<BufReader<BufWriter<T>>>,
}

/// Builder for a WebSocket transport [`Sender`] and [`Receiver`] pair.
#[derive(Debug)]
pub struct WsTransportClientBuilder {
	#[cfg(feature = "tls")]
	/// What certificate store to use
	pub certificate_store: CertificateStore,
	/// Timeout for the connection.
	pub connection_timeout: Duration,
	/// Custom headers to pass during the HTTP handshake.
	pub headers: http::HeaderMap,
	/// Max request payload size
	pub max_request_size: u32,
	/// Max response payload size
	pub max_response_size: u32,
	/// Max frame size
	pub max_frame_size: Option<u32>,
	/// Max number of redirections.
	pub max_redirections: usize,
	/// TCP no delay.
	pub tcp_no_delay: bool,
}

impl Default for WsTransportClientBuilder {
	fn default() -> Self {
		Self {
			#[cfg(feature = "tls")]
			certificate_store: CertificateStore::Native,
			max_request_size: TEN_MB_SIZE_BYTES,
			max_response_size: TEN_MB_SIZE_BYTES,
			max_frame_size: None,
			connection_timeout: Duration::from_secs(10),
			headers: http::HeaderMap::new(),
			max_redirections: 5,
			tcp_no_delay: true,
		}
	}
}

impl WsTransportClientBuilder {
	/// Force to use a custom certificate store.
	///
	/// # Optional
	///
	/// This requires the optional `tls` feature.
	#[cfg(feature = "tls")]
	pub fn with_custom_cert_store(mut self, cfg: CustomCertStore) -> Self {
		self.certificate_store = CertificateStore::Custom(cfg);
		self
	}

	/// Set the maximum size of a request in bytes. Default is 10 MiB.
	pub fn max_request_size(mut self, size: u32) -> Self {
		self.max_request_size = size;
		self
	}

	/// Set the maximum size of a response in bytes. Default is 10 MiB.
	pub fn max_response_size(mut self, size: u32) -> Self {
		self.max_response_size = size;
		self
	}

	/// Set the maximum size of a frame in bytes (default is none).
	pub fn max_frame_size(mut self, size: u32) -> Self {
		self.max_frame_size = Some(size);
		self
	}

	/// Set connection timeout for the handshake (default is 10 seconds).
	pub fn connection_timeout(mut self, timeout: Duration) -> Self {
		self.connection_timeout = timeout;
		self
	}

	/// Set a custom header passed to the server during the handshake (default is none).
	///
	/// The caller is responsible for checking that the headers do not conflict or are duplicated.
	pub fn set_headers(mut self, headers: http::HeaderMap) -> Self {
		self.headers = headers;
		self
	}

	/// Set the max number of redirections to perform until a connection is regarded as failed.
	/// (default is 5).
	pub fn max_redirections(mut self, redirect: usize) -> Self {
		self.max_redirections = redirect;
		self
	}
}

/// Stream mode, either plain TCP or TLS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
	/// Plain mode (`ws://` URL).
	Plain,
	/// TLS mode (`wss://` URL).
	Tls,
}

/// Error that can happen during the WebSocket handshake.
///
/// If multiple IP addresses are attempted, only the last error is returned, similar to how
/// [`std::net::TcpStream::connect`] behaves.
#[derive(Debug, Error)]
pub enum WsHandshakeError {
	/// Failed to load system certs
	#[error("Failed to load system certs: {0}")]
	CertificateStore(io::Error),

	/// Invalid URL.
	#[error("Invalid URL: {0}")]
	Url(Cow<'static, str>),

	/// Error when opening the TCP socket.
	#[error("Error when opening the TCP socket: {0}")]
	Io(io::Error),

	/// Error in the transport layer.
	#[error("{0}")]
	Transport(#[source] soketto::handshake::Error),

	/// Server rejected the handshake.
	#[error("Connection rejected with status code: {status_code}")]
	Rejected {
		/// HTTP status code that the server returned.
		status_code: u16,
	},

	/// Server redirected to other location.
	#[error("Connection redirected with status code: {status_code} and location: {location}")]
	Redirected {
		/// HTTP status code that the server returned.
		status_code: u16,
		/// The location URL redirected to.
		location: String,
	},

	/// Timeout while trying to connect.
	#[error("Connection timeout exceeded: {0:?}")]
	Timeout(Duration),

	/// Failed to resolve IP addresses for this hostname.
	#[error("Failed to resolve IP addresses for this hostname: {0}")]
	ResolutionFailed(io::Error),

	/// Couldn't find any IP address for this hostname.
	#[error("No IP address found for this hostname: {0}")]
	NoAddressFound(String),
}

/// Error that can occur when reading or sending messages on an established connection.
#[derive(Debug, Error)]
pub enum WsError {
	/// Error in the WebSocket connection.
	#[error("{0}")]
	Connection(#[source] soketto::connection::Error),
	/// Message was too large.
	#[error("The message was too large")]
	MessageTooLarge,
	/// Connection was closed.
	#[error("Connection was closed: {0:?}")]
	Closed(CloseReason),
}

impl<T> TransportSenderT for Sender<T>
where
	T: futures_util::io::AsyncRead + futures_util::io::AsyncWrite + Send + Unpin + 'static,
{
	type Error = WsError;

	/// Sends out a request. Returns a `Future` that finishes when the request has been
	/// successfully sent.
	fn send(&mut self, body: String) -> impl Future<Output = Result<(), Self::Error>> + Send {
		async {
			if body.len() > self.max_request_size as usize {
				return Err(WsError::MessageTooLarge);
			}

			self.inner.send_text(body).await?;
			self.inner.flush().await?;
			Ok(())
		}
	}

	/// Sends out a ping request. Returns a `Future` that finishes when the request has been
	/// successfully sent.
	fn send_ping(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
		async {
			tracing::debug!(target: LOG_TARGET, "Send ping");
			// Submit empty slice as "optional" parameter.
			let slice: &[u8] = &[];
			// Byte slice fails if the provided slice is larger than 125 bytes.
			let byte_slice = ByteSlice125::try_from(slice).expect("Empty slice should fit into ByteSlice125");

			self.inner.send_ping(byte_slice).await?;
			self.inner.flush().await?;
			Ok(())
		}
	}

	/// Send a close message and close the connection.
	fn close(&mut self) -> impl Future<Output = Result<(), WsError>> + Send {
		async { self.inner.close().await.map_err(Into::into) }
	}
}

impl<T> TransportReceiverT for Receiver<T>
where
	T: futures_util::io::AsyncRead + futures_util::io::AsyncWrite + Unpin + Send + 'static,
{
	type Error = WsError;

	/// Returns a `Future` resolving when the server sent us something back.
	fn receive(&mut self) -> impl Future<Output = Result<ReceivedMessage, Self::Error>> + Send {
		async {
			let mut message = Vec::new();

			match self.inner.receive(&mut message).await? {
				Incoming::Data(Data::Text(_)) => {
					let s = String::from_utf8(message).map_err(|err| WsError::Connection(Utf8(err.utf8_error())))?;
					Ok(ReceivedMessage::Text(s))
				}
				Incoming::Data(Data::Binary(_)) => Ok(ReceivedMessage::Bytes(message)),
				Incoming::Pong(_) => Ok(ReceivedMessage::Pong),
				Incoming::Closed(c) => Err(WsError::Closed(c)),
			}
		}
	}
}

impl WsTransportClientBuilder {
	/// Try to establish the connection.
	///
	/// Uses the default connection over TCP.
	pub async fn build(
		self,
		uri: Url,
	) -> Result<(Sender<Compat<EitherStream>>, Receiver<Compat<EitherStream>>), WsHandshakeError> {
		self.try_connect_over_tcp(uri).await
	}

	/// Try to establish the connection over the given data stream.
	pub async fn build_with_stream<T>(
		self,
		uri: Url,
		data_stream: T,
	) -> Result<(Sender<Compat<T>>, Receiver<Compat<T>>), WsHandshakeError>
	where
		T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
	{
		let target: Target = uri.try_into()?;
		self.try_connect(&target, data_stream.compat()).await
	}

	#[cfg(feature = "tls")]
	fn tls_connector(&self, target: &Target) -> Result<Option<tokio_rustls::TlsConnector>, WsHandshakeError> {
		// Make sure that the TLS provider is set. If not, set a default one.
		// Otherwise, creating `tls` configuration may panic if there are multiple
		// providers available due to `rustls` features (e.g. both `ring` and `aws-lc-rs`).
		// Function returns an error if the provider is already installed, and we're fine with it.
		let _ = rustls::crypto::ring::default_provider().install_default();

		let connector = match target._mode {
			Mode::Tls => Some(build_tls_config(&self.certificate_store)?),
			Mode::Plain => None,
		};
		Ok(connector)
	}

	// Try to establish the connection over TCP.
	async fn try_connect_over_tcp(
		&self,
		uri: Url,
	) -> Result<(Sender<Compat<EitherStream>>, Receiver<Compat<EitherStream>>), WsHandshakeError> {
		let mut target: Target = uri.clone().try_into()?;
		let mut err = None;

		// Only build TLS connector if `wss` in URL.
		#[cfg(feature = "tls")]
		let mut connector = self.tls_connector(&target)?;

		// The sockaddrs might get reused if the server replies with a relative URI.
		let mut target_sockaddrs = resolve_sockaddrs(&uri, self.connection_timeout).await?;

		for _ in 0..self.max_redirections {
			tracing::debug!(target: LOG_TARGET, "Connecting to target: {:?}", target);

			let sockaddrs = std::mem::take(&mut target_sockaddrs);

			for sockaddr in &sockaddrs {
				#[cfg(feature = "tls")]
				let tcp_stream = match connect(
					*sockaddr,
					self.connection_timeout,
					&target.host,
					connector.as_ref(),
					self.tcp_no_delay,
				)
				.await
				{
					Ok(stream) => stream,
					Err(e) => {
						tracing::debug!(target: LOG_TARGET, "Failed to connect to sockaddr: {:?}", sockaddr);
						err = Some(Err(e));
						continue;
					}
				};

				#[cfg(not(feature = "tls"))]
				let tcp_stream = match connect(*sockaddr, self.connection_timeout).await {
					Ok(stream) => stream,
					Err(e) => {
						tracing::debug!(target: LOG_TARGET, "Failed to connect to sockaddr: {:?}", sockaddr);
						err = Some(Err(e));
						continue;
					}
				};

				match self.try_connect(&target, tcp_stream.compat()).await {
					Ok(result) => return Ok(result),

					Err(WsHandshakeError::Redirected { status_code, location }) => {
						tracing::debug!(target: LOG_TARGET, "Redirection: status_code: {}, location: {}", status_code, location);
						match Url::parse(&location) {
							// redirection with absolute path => need to lookup.
							Ok(uri) => {
								// Absolute URI.
								target_sockaddrs = resolve_sockaddrs(&uri, self.connection_timeout).await?;

								target = uri.try_into().map_err(|e| {
									tracing::debug!(target: LOG_TARGET, "Redirection failed: {:?}", e);
									e
								})?;

								// Only build TLS connector if `wss` in redirection URL.
								#[cfg(feature = "tls")]
								match target._mode {
									Mode::Tls if connector.is_none() => {
										connector = Some(build_tls_config(&self.certificate_store)?);
									}
									Mode::Tls => (),
									// Drop connector if it was configured previously.
									Mode::Plain => {
										connector = None;
									}
								};
							}

							// Relative URI such as `/foo/bar.html` or `cake.html`
							Err(url::ParseError::RelativeUrlWithoutBase) => {
								// Replace the entire path_and_query if `location` starts with `/` or `//`.
								if location.starts_with('/') {
									target.path_and_query = location;
								} else {
									match target.path_and_query.rfind('/') {
										Some(offset) => target.path_and_query.replace_range(offset + 1.., &location),
										None => {
											let e = format!(
												"path_and_query: {location}; this is a bug it must contain `/` please open issue"
											);
											err = Some(Err(WsHandshakeError::Url(e.into())));
											continue;
										}
									};
								}
								target_sockaddrs = sockaddrs;
								break;
							}

							Err(e) => {
								err = Some(Err(WsHandshakeError::Url(e.to_string().into())));
							}
						};
					}

					Err(e) => {
						err = Some(Err(e));
					}
				};
			}
		}
		err.unwrap_or(Err(WsHandshakeError::NoAddressFound(target.host)))
	}

	/// Try to establish the handshake over the given data stream.
	async fn try_connect<T>(
		&self,
		target: &Target,
		data_stream: T,
	) -> Result<(Sender<T>, Receiver<T>), WsHandshakeError>
	where
		T: futures_util::AsyncRead + futures_util::AsyncWrite + Unpin,
	{
		let mut client = WsHandshakeClient::new(
			BufReader::new(BufWriter::new(data_stream)),
			&target.host_header,
			&target.path_and_query,
		);

		let headers: Vec<_> = match &target.basic_auth {
			Some(basic_auth) if !self.headers.contains_key(http::header::AUTHORIZATION) => {
				let it1 =
					self.headers.iter().map(|(key, value)| Header { name: key.as_str(), value: value.as_bytes() });
				let it2 = std::iter::once(Header {
					name: http::header::AUTHORIZATION.as_str(),
					value: basic_auth.as_bytes(),
				});

				it1.chain(it2).collect()
			}
			_ => {
				self.headers.iter().map(|(key, value)| Header { name: key.as_str(), value: value.as_bytes() }).collect()
			}
		};

		client.set_headers(&headers);

		// Perform the initial handshake.
		let response = tokio::time::timeout(self.connection_timeout, client.handshake())
			.await
			.map_err(|_| WsHandshakeError::Timeout(self.connection_timeout))?;

		match response {
			Ok(ServerResponse::Accepted { .. }) => {
				tracing::debug!(target: LOG_TARGET, "Connection established to target: {:?}", target);
				let mut builder = client.into_builder();
				builder.set_max_message_size(self.max_response_size as usize);
				// Use the max frame size if any, otherwise let the underlying code use appropriate defaults.
				if let Some(max_frame_size) = self.max_frame_size {
					builder.set_max_frame_size(max_frame_size as usize);
				}
				let (sender, receiver) = builder.finish();
				Ok((Sender { inner: sender, max_request_size: self.max_request_size }, Receiver { inner: receiver }))
			}

			Ok(ServerResponse::Rejected { status_code }) => {
				tracing::debug!(target: LOG_TARGET, "Connection rejected: {:?}", status_code);
				Err(WsHandshakeError::Rejected { status_code })
			}

			Ok(ServerResponse::Redirect { status_code, location }) => {
				tracing::debug!(target: LOG_TARGET, "Redirection: status_code: {}, location: {}", status_code, location);
				Err(WsHandshakeError::Redirected { status_code, location })
			}

			Err(e) => Err(e.into()),
		}
	}
}

/// Resolve the socket addresses for `uri` with async DNS, bounded by `timeout_dur`.
async fn resolve_sockaddrs(uri: &Url, timeout_dur: Duration) -> Result<Vec<SocketAddr>, WsHandshakeError> {
	let resolve = async {
		let port =
			uri.port_or_known_default().ok_or_else(|| WsHandshakeError::Url("No port number in the URL".into()))?;

		match uri.host() {
			Some(url::Host::Domain(domain)) => {
				tokio::net::lookup_host((domain, port)).await.map(|addrs| addrs.collect()).map_err(|e| {
					tracing::debug!(target: LOG_TARGET, "DNS resolution failed for {domain}: {e:?}");
					WsHandshakeError::ResolutionFailed(e)
				})
			}
			Some(url::Host::Ipv4(ip)) => Ok(vec![SocketAddr::from((ip, port))]),
			Some(url::Host::Ipv6(ip)) => Ok(vec![SocketAddr::from((ip, port))]),
			None => Err(WsHandshakeError::Url("No host name in the URL".into())),
		}
	};

	tokio::time::timeout(timeout_dur, resolve).await.map_err(|_| WsHandshakeError::Timeout(timeout_dur))?
}

#[cfg(feature = "tls")]
async fn connect(
	sockaddr: SocketAddr,
	timeout_dur: Duration,
	host: &str,
	tls_connector: Option<&tokio_rustls::TlsConnector>,
	tcp_no_delay: bool,
) -> Result<EitherStream, WsHandshakeError> {
	let socket = TcpStream::connect(sockaddr);
	let timeout = tokio::time::sleep(timeout_dur);
	tokio::select! {
		socket = socket => {
			let socket = socket?;
			if let Err(err) = socket.set_nodelay(tcp_no_delay) {
				tracing::warn!(target: LOG_TARGET, "set nodelay failed: {:?}", err);
			}
			match tls_connector {
				None => Ok(EitherStream::Plain(socket)),
				Some(connector) => {
					let server_name: rustls_pki_types::ServerName = host.try_into().map_err(|e| WsHandshakeError::Url(format!("Invalid host: {host} {e:?}").into()))?;
					let tls_stream = connector.connect(server_name.to_owned(), socket).await?;
					Ok(EitherStream::Tls(tls_stream))
				}
			}
		}
		_ = timeout => Err(WsHandshakeError::Timeout(timeout_dur))
	}
}

#[cfg(not(feature = "tls"))]
async fn connect(sockaddr: SocketAddr, timeout_dur: Duration) -> Result<EitherStream, WsHandshakeError> {
	let socket = TcpStream::connect(sockaddr);
	let timeout = tokio::time::sleep(timeout_dur);
	tokio::select! {
		socket = socket => {
			let socket = socket?;
			if let Err(err) = socket.set_nodelay(true) {
				tracing::warn!(target: LOG_TARGET, "set nodelay failed: {:?}", err);
			}
			Ok(EitherStream::Plain(socket))
		}
		_ = timeout => Err(WsHandshakeError::Timeout(timeout_dur))
	}
}

impl From<io::Error> for WsHandshakeError {
	fn from(err: io::Error) -> WsHandshakeError {
		WsHandshakeError::Io(err)
	}
}

impl From<soketto::handshake::Error> for WsHandshakeError {
	fn from(err: soketto::handshake::Error) -> WsHandshakeError {
		WsHandshakeError::Transport(err)
	}
}

impl From<soketto::connection::Error> for WsError {
	fn from(err: soketto::connection::Error) -> Self {
		WsError::Connection(err)
	}
}

/// Represents a verified remote WebSocket address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
	/// The host name (domain or IP address).
	host: String,
	/// The Host request header specifies the host and port number of the server to which the request is being sent.
	host_header: String,
	/// WebSocket stream mode, see [`Mode`] for further documentation.
	_mode: Mode,
	/// The path and query parts from an URL.
	path_and_query: String,
	/// Optional <username:password> from an URL.
	basic_auth: Option<HeaderValue>,
}

impl TryFrom<url::Url> for Target {
	type Error = WsHandshakeError;

	fn try_from(url: Url) -> Result<Self, Self::Error> {
		let _mode = match url.scheme() {
			"ws" => Mode::Plain,
			#[cfg(feature = "tls")]
			"wss" => Mode::Tls,
			invalid_scheme => {
				#[cfg(feature = "tls")]
				let err = format!("`{invalid_scheme}` not supported, expects 'ws' or 'wss'");
				#[cfg(not(feature = "tls"))]
				let err = format!("`{invalid_scheme}` not supported, expects 'ws' ('wss' requires the tls feature)");
				return Err(WsHandshakeError::Url(err.into()));
			}
		};
		let host = url.host_str().map(ToOwned::to_owned).ok_or_else(|| WsHandshakeError::Url("Invalid host".into()))?;

		let mut path_and_query = url.path().to_owned();
		if let Some(query) = url.query() {
			path_and_query.push('?');
			path_and_query.push_str(query);
		}

		let basic_auth = if let Some(pwd) = url.password() {
			let digest = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", url.username(), pwd));
			let val = HeaderValue::from_str(&format!("Basic {digest}"))
				.map_err(|_| WsHandshakeError::Url("Header value `authorization basic user:pwd` invalid".into()))?;

			Some(val)
		} else {
			None
		};

		let host_header = if let Some(port) = url.port() { format!("{host}:{port}") } else { host.to_string() };

		Ok(Self { host, host_header, _mode, path_and_query: path_and_query.to_string(), basic_auth })
	}
}

// NOTE: this is slow and should be used sparingly.
#[cfg(feature = "tls")]
fn build_tls_config(cert_store: &CertificateStore) -> Result<tokio_rustls::TlsConnector, WsHandshakeError> {
	let config = match cert_store {
		#[cfg(feature = "tls-rustls-platform-verifier")]
		CertificateStore::Native => {
			use rustls_platform_verifier::ConfigVerifierExt;

			rustls::ClientConfig::with_platform_verifier()
		}
		#[cfg(not(feature = "tls-rustls-platform-verifier"))]
		CertificateStore::Native => {
			return Err(WsHandshakeError::CertificateStore(io::Error::new(
				io::ErrorKind::Other,
				"Native certificate store not supported, either call `Builder::with_custom_cert_store` or enable the `tls-rustls-platform-verifier` feature.",
			)));
		}
		CertificateStore::Custom(cfg) => cfg.clone(),
	};

	Ok(std::sync::Arc::new(config).into())
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use http::HeaderValue;

	use super::{Mode, Target, Url, WsHandshakeError, resolve_sockaddrs};

	fn assert_ws_target(
		target: Target,
		host: &str,
		host_header: &str,
		mode: Mode,
		path_and_query: &str,
		basic_auth: Option<HeaderValue>,
	) {
		assert_eq!(&target.host, host);
		assert_eq!(&target.host_header, host_header);
		assert_eq!(target._mode, mode);
		assert_eq!(&target.path_and_query, path_and_query);
		assert_eq!(target.basic_auth, basic_auth);
	}

	fn parse_target(uri: &str) -> Result<Target, WsHandshakeError> {
		Url::parse(uri).map_err(|e| WsHandshakeError::Url(e.to_string().into()))?.try_into()
	}

	#[test]
	fn ws_works_with_port() {
		let target = parse_target("ws://127.0.0.1:9933").unwrap();
		assert_ws_target(target, "127.0.0.1", "127.0.0.1:9933", Mode::Plain, "/", None);
	}

	#[cfg(feature = "tls")]
	#[test]
	fn wss_works_with_port() {
		let target = parse_target("wss://kusama-rpc.polkadot.io:9999").unwrap();
		assert_ws_target(target, "kusama-rpc.polkadot.io", "kusama-rpc.polkadot.io:9999", Mode::Tls, "/", None);
	}

	#[cfg(not(feature = "tls"))]
	#[test]
	fn wss_fails_with_tls_feature() {
		let err = parse_target("wss://kusama-rpc.polkadot.io").unwrap_err();
		assert!(matches!(err, WsHandshakeError::Url(_)));
	}

	#[test]
	fn faulty_url_scheme() {
		let err = parse_target("http://kusama-rpc.polkadot.io:443").unwrap_err();
		assert!(matches!(err, WsHandshakeError::Url(_)));
	}

	#[test]
	fn faulty_port() {
		let err = parse_target("ws://127.0.0.1:-43").unwrap_err();
		assert!(matches!(err, WsHandshakeError::Url(_)));
		let err = parse_target("ws://127.0.0.1:99999").unwrap_err();
		assert!(matches!(err, WsHandshakeError::Url(_)));
	}

	#[test]
	fn url_with_path_works() {
		let target = parse_target("ws://127.0.0.1/my-special-path").unwrap();
		assert_ws_target(target, "127.0.0.1", "127.0.0.1", Mode::Plain, "/my-special-path", None);
	}

	#[test]
	fn url_with_query_works() {
		let target = parse_target("ws://127.0.0.1/my?name1=value1&name2=value2").unwrap();
		assert_ws_target(target, "127.0.0.1", "127.0.0.1", Mode::Plain, "/my?name1=value1&name2=value2", None);
	}

	#[test]
	fn url_with_fragment_is_ignored() {
		let target = parse_target("ws://127.0.0.1:/my.htm#ignore").unwrap();
		assert_ws_target(target, "127.0.0.1", "127.0.0.1", Mode::Plain, "/my.htm", None);
	}

	#[cfg(feature = "tls")]
	#[test]
	fn wss_default_port_is_omitted() {
		let target = parse_target("wss://127.0.0.1:443").unwrap();
		assert_ws_target(target, "127.0.0.1", "127.0.0.1", Mode::Tls, "/", None);
	}

	#[test]
	fn ws_default_port_is_omitted() {
		let target = parse_target("ws://127.0.0.1:80").unwrap();
		assert_ws_target(target, "127.0.0.1", "127.0.0.1", Mode::Plain, "/", None);
	}

	#[test]
	fn ws_with_username_and_password() {
		use base64::Engine;

		let target = parse_target("ws://user:pwd@127.0.0.1").unwrap();
		let digest = base64::engine::general_purpose::STANDARD.encode("user:pwd");
		let basic_auth = HeaderValue::from_str(&format!("Basic {digest}")).unwrap();

		assert_ws_target(target, "127.0.0.1", "127.0.0.1", Mode::Plain, "/", Some(basic_auth));
	}

	// A slow or broken resolver must not stall the connection: DNS resolution is bounded by the
	// connection timeout. `.invalid` is a reserved TLD (RFC 6761) that never resolves.
	#[tokio::test]
	async fn dns_resolution_is_bounded_by_timeout() {
		let url = Url::parse("ws://host.invalid").unwrap();
		let err = resolve_sockaddrs(&url, Duration::ZERO).await.unwrap_err();
		assert!(matches!(err, WsHandshakeError::Timeout(_)));
	}

	#[tokio::test]
	async fn ip_literals_and_default_ports_resolve_without_dns() {
		for uri in ["ws://127.0.0.1:9933", "ws://[::1]:9933", "ws://127.0.0.1"] {
			let url = Url::parse(uri).unwrap();
			let resolved = resolve_sockaddrs(&url, Duration::from_secs(1)).await.unwrap();
			assert_eq!(resolved, url.socket_addrs(|| None).unwrap());
		}
	}
}
