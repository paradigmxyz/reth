//! This contains an authenticated rpc transport that can be used to send engine API newPayload
//! requests.

use std::sync::Arc;

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use alloy_pubsub::{PubSubConnect, PubSubFrontend};
use alloy_rpc_types_engine::{Claims, JwtSecret};
use alloy_transport::{Authorization, TransportError, TransportFut};
use alloy_transport_http::{reqwest::Url, Http, ReqwestTransport};
use alloy_transport_ipc::IpcConnect;
use alloy_transport_ws::WsConnect;
use reqwest::header::HeaderValue;
use std::task::{Context, Poll};
use tokio::sync::RwLock;
use tower::Service;

/// An enum representing the different transports that can be used to connect to a runtime.
/// Only meant to be used internally by [AuthenticatedTransport].
#[derive(Clone, Debug)]
pub enum InnerTransport {
    /// HTTP transport
    Http(ReqwestTransport),
    /// WebSocket transport
    Ws(PubSubFrontend),
    /// IPC transport
    Ipc(PubSubFrontend),
}

#[derive(Debug, Clone)]
pub struct AuthenticatedTransport {
    /// The inner actual transport used.
    inner: Arc<RwLock<InnerTransport>>,
    /// The URL to connect to.
    url: Url,
    /// The jwt
    jwt: String,
}

/// An error that can occur when creating an authenticated transport.
#[derive(Debug, thiserror::Error)]
pub enum AuthenticatedTransportError {
    /// The URL is invalid.
    #[error("The URL is invalid")]
    InvalidUrl,
    /// Failed to lock transport
    #[error("Failed to lock transport")]
    LockFailed,
    /// The JWT is invalid.
    #[error("The JWT is invalid: {0}")]
    InvalidJwt(String),
    /// The transport failed to connect.
    #[error("The transport failed to connect to {1}, transport error: {0}")]
    TransportError(TransportError, String),
    /// The http client could not be built.
    #[error("The http client could not be built")]
    HttpConstructionError(reqwest::Error),
    /// The scheme is invalid.
    #[error("The URL scheme is invalid: {0}")]
    BadScheme(String),
}

impl AuthenticatedTransport {
    /// Create a new builder with the given URL.
    pub async fn connect(url: Url, jwt: String) -> Result<Self, AuthenticatedTransportError> {
        match url.scheme() {
            "http" | "https" => Self::connect_http(url, jwt).await,
            "ws" | "wss" => Self::connect_ws(url, jwt).await,
            "file" => Self::connect_ipc(url, jwt).await,
            _ => Err(AuthenticatedTransportError::BadScheme(url.scheme().to_string())),
        }
    }

    /// Connects to an HTTP [alloy_transport_http::Http] transport.
    async fn connect_http(url: Url, jwt: String) -> Result<Self, AuthenticatedTransportError> {
        let mut client_builder =
            reqwest::Client::builder().tls_built_in_root_certs(url.scheme() == "https");
        let mut headers = reqwest::header::HeaderMap::new();

        // Add the JWT it to the headers if we can decode it.
        let auth = build_auth(jwt.clone())
            .map_err(|e| AuthenticatedTransportError::InvalidJwt(e.to_string()))?;

        let mut auth_value: HeaderValue =
            HeaderValue::from_str(&auth.to_string()).expect("Header should be valid string");
        auth_value.set_sensitive(true);

        headers.insert(reqwest::header::AUTHORIZATION, auth_value);
        client_builder = client_builder.default_headers(headers);

        let client =
            client_builder.build().map_err(AuthenticatedTransportError::HttpConstructionError)?;

        let inner = InnerTransport::Http(Http::with_client(client, url.clone()));

        Ok(Self { inner: Arc::new(RwLock::new(inner)), url, jwt })
    }

    /// Connects to a WebSocket [alloy_transport_ws::WsConnect] transport.
    async fn connect_ws(url: Url, jwt: String) -> Result<Self, AuthenticatedTransportError> {
        // Add the JWT it to the headers if we can decode it.
        let auth = build_auth(jwt.clone())
            .map_err(|e| AuthenticatedTransportError::InvalidJwt(e.to_string()))?;

        let inner = WsConnect { url: url.to_string(), auth: Some(auth) }
            .into_service()
            .await
            .map(InnerTransport::Ws)
            .map_err(|e| AuthenticatedTransportError::TransportError(e, url.to_string()))?;

        Ok(Self { inner: Arc::new(RwLock::new(inner)), url, jwt })
    }

    /// Connects to an IPC [alloy_transport_ipc::IpcConnect] transport.
    async fn connect_ipc(url: Url, jwt: String) -> Result<Self, AuthenticatedTransportError> {
        // IPC, even for engine, typically does not require auth because it's local
        let inner = IpcConnect::new(url.to_string())
            .into_service()
            .await
            .map(InnerTransport::Ipc)
            .map_err(|e| AuthenticatedTransportError::TransportError(e, url.to_string()))?;

        Ok(Self { inner: Arc::new(RwLock::new(inner)), url, jwt })
    }

    /// Sends a request using the underlying transport.
    ///
    /// For sending the actual request, this action is delegated down to the underlying transport
    /// through Tower's [tower::Service::call]. See tower's [tower::Service] trait for more
    /// information.
    fn request(&self, req: RequestPacket) -> TransportFut<'static> {
        let this = self.clone();
        Box::pin(async move {
            let inner = this.inner.read().await;
            match *inner {
                InnerTransport::Http(ref http) => {
                    let mut http = http;
                    http.call(req)
                }
                InnerTransport::Ws(ref ws) => {
                    let mut ws = ws;
                    ws.call(req)
                }
                InnerTransport::Ipc(ref ipc) => {
                    let mut ipc = ipc;
                    ipc.call(req)
                }
            }
            .await
        })
    }
}

fn build_auth(jwt: String) -> eyre::Result<Authorization> {
    // Decode jwt from hex, then generate claims (iat with current timestamp)
    let secret = JwtSecret::from_hex(jwt)?;
    let claims = Claims::default();
    let token = secret.encode(&claims)?;

    let auth = Authorization::Bearer(token);

    Ok(auth)
}

impl tower::Service<RequestPacket> for AuthenticatedTransport {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        self.request(req)
    }
}

impl tower::Service<RequestPacket> for &AuthenticatedTransport {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        self.request(req)
    }
}
