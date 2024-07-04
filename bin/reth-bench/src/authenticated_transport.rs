//! This contains an authenticated rpc transport that can be used to send engine API newPayload
//! requests.

use std::sync::Arc;

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use alloy_pubsub::{PubSubConnect, PubSubFrontend};
use alloy_rpc_types_engine::{Claims, JwtSecret};
use alloy_transport::{
    utils::guess_local_url, Authorization, Pbf, TransportConnect, TransportError,
    TransportErrorKind, TransportFut,
};
use alloy_transport_http::{reqwest::Url, Http, ReqwestTransport};
use alloy_transport_ipc::IpcConnect;
use alloy_transport_ws::WsConnect;
use futures::FutureExt;
use reqwest::header::HeaderValue;
use std::task::{Context, Poll};
use tokio::sync::RwLock;
use tower::Service;

/// An enum representing the different transports that can be used to connect to a runtime.
/// Only meant to be used internally by [`AuthenticatedTransport`].
#[derive(Clone, Debug)]
pub enum InnerTransport {
    /// HTTP transport
    Http(ReqwestTransport),
    /// `WebSocket` transport
    Ws(PubSubFrontend),
    /// IPC transport
    Ipc(PubSubFrontend),
}

impl InnerTransport {
    /// Connects to a transport based on the given URL and JWT. Returns an [`InnerTransport`] and
    /// the [`Claims`] generated from the jwt.
    async fn connect(
        url: Url,
        jwt: JwtSecret,
    ) -> Result<(Self, Claims), AuthenticatedTransportError> {
        match url.scheme() {
            "http" | "https" => Self::connect_http(url, jwt),
            "ws" | "wss" => Self::connect_ws(url, jwt).await,
            "file" => Ok((Self::connect_ipc(url).await?, Claims::default())),
            _ => Err(AuthenticatedTransportError::BadScheme(url.scheme().to_string())),
        }
    }

    /// Connects to an HTTP [`alloy_transport_http::Http`] transport. Returns an [`InnerTransport`]
    /// and the [Claims] generated from the jwt.
    fn connect_http(
        url: Url,
        jwt: JwtSecret,
    ) -> Result<(Self, Claims), AuthenticatedTransportError> {
        let mut client_builder =
            reqwest::Client::builder().tls_built_in_root_certs(url.scheme() == "https");
        let mut headers = reqwest::header::HeaderMap::new();

        // Add the JWT it to the headers if we can decode it.
        let (auth, claims) =
            build_auth(jwt).map_err(|e| AuthenticatedTransportError::InvalidJwt(e.to_string()))?;

        let mut auth_value: HeaderValue =
            HeaderValue::from_str(&auth.to_string()).expect("Header should be valid string");
        auth_value.set_sensitive(true);

        headers.insert(reqwest::header::AUTHORIZATION, auth_value);
        client_builder = client_builder.default_headers(headers);

        let client =
            client_builder.build().map_err(AuthenticatedTransportError::HttpConstructionError)?;

        let inner = Self::Http(Http::with_client(client, url));
        Ok((inner, claims))
    }

    /// Connects to a `WebSocket` [`alloy_transport_ws::WsConnect`] transport. Returns an
    /// [`InnerTransport`] and the [`Claims`] generated from the jwt.
    async fn connect_ws(
        url: Url,
        jwt: JwtSecret,
    ) -> Result<(Self, Claims), AuthenticatedTransportError> {
        // Add the JWT it to the headers if we can decode it.
        let (auth, claims) =
            build_auth(jwt).map_err(|e| AuthenticatedTransportError::InvalidJwt(e.to_string()))?;

        let inner = WsConnect { url: url.to_string(), auth: Some(auth) }
            .into_service()
            .await
            .map(Self::Ws)
            .map_err(|e| AuthenticatedTransportError::TransportError(e, url.to_string()))?;

        Ok((inner, claims))
    }

    /// Connects to an IPC [`alloy_transport_ipc::IpcConnect`] transport. Returns an
    /// [`InnerTransport`]. Does not return any [`Claims`] because IPC does not require them.
    async fn connect_ipc(url: Url) -> Result<Self, AuthenticatedTransportError> {
        // IPC, even for engine, typically does not require auth because it's local
        IpcConnect::new(url.to_string())
            .into_service()
            .await
            .map(InnerTransport::Ipc)
            .map_err(|e| AuthenticatedTransportError::TransportError(e, url.to_string()))
    }
}

/// An authenticated transport that can be used to send requests that contain a jwt bearer token.
#[derive(Debug, Clone)]
pub struct AuthenticatedTransport {
    /// The inner actual transport used.
    ///
    /// Also contains the current claims being used. This is used to determine whether or not we
    /// should create another client.
    inner_and_claims: Arc<RwLock<(InnerTransport, Claims)>>,
    /// The current jwt being used. This is so we can recreate claims.
    jwt: JwtSecret,
    /// The current URL being used. This is so we can recreate the client if needed.
    url: Url,
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
    pub async fn connect(url: Url, jwt: JwtSecret) -> Result<Self, AuthenticatedTransportError> {
        let (inner, claims) = InnerTransport::connect(url.clone(), jwt).await?;
        Ok(Self { inner_and_claims: Arc::new(RwLock::new((inner, claims))), jwt, url })
    }

    /// Sends a request using the underlying transport.
    ///
    /// For sending the actual request, this action is delegated down to the underlying transport
    /// through Tower's [`tower::Service::call`]. See tower's [`tower::Service`] trait for more
    /// information.
    fn request(&self, req: RequestPacket) -> TransportFut<'static> {
        let this = self.clone();

        Box::pin(async move {
            let mut inner_and_claims = this.inner_and_claims.write().await;

            // shift the iat forward by one second so there is some buffer time
            let mut shifted_claims = inner_and_claims.1;
            shifted_claims.iat -= 1;

            // if the claims are out of date, reset the inner transport
            if !shifted_claims.is_within_time_window() {
                let (new_inner, new_claims) =
                    InnerTransport::connect(this.url.clone(), this.jwt).await.map_err(|e| {
                        TransportError::Transport(TransportErrorKind::Custom(Box::new(e)))
                    })?;
                *inner_and_claims = (new_inner, new_claims);
            }

            match inner_and_claims.0 {
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
                    // we don't need to recreate the client for IPC
                    ipc.call(req)
                }
            }
            .await
        })
    }
}

fn build_auth(secret: JwtSecret) -> eyre::Result<(Authorization, Claims)> {
    // Generate claims (iat with current timestamp), this happens by default using the Default trait
    // for Claims.
    let claims = Claims::default();
    let token = secret.encode(&claims)?;
    let auth = Authorization::Bearer(token);

    Ok((auth, claims))
}

/// This specifies how to connect to an authenticated transport.
#[derive(Clone, Debug)]
pub struct AuthenticatedTransportConnect {
    /// The URL to connect to.
    url: Url,
    /// The JWT secret used to authenticate the transport.
    jwt: JwtSecret,
}

impl AuthenticatedTransportConnect {
    /// Create a new builder with the given URL.
    pub const fn new(url: Url, jwt: JwtSecret) -> Self {
        Self { url, jwt }
    }
}

impl TransportConnect for AuthenticatedTransportConnect {
    type Transport = AuthenticatedTransport;

    fn is_local(&self) -> bool {
        guess_local_url(&self.url)
    }

    fn get_transport<'a: 'b, 'b>(&'a self) -> Pbf<'b, Self::Transport, TransportError> {
        AuthenticatedTransport::connect(self.url.clone(), self.jwt)
            .map(|res| match res {
                Ok(transport) => Ok(transport),
                Err(err) => {
                    Err(TransportError::Transport(TransportErrorKind::Custom(Box::new(err))))
                }
            })
            .boxed()
    }
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
