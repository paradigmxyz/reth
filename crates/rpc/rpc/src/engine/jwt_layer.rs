use http::{HeaderMap, Request, Response, StatusCode};

use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};
use tracing::error;

use crate::JwtSecret;

/// This is an Http middleware layer that acts as an
/// interceptor for `Authorization: Bearer <JWT-TOKEN>`
/// headers. Incoming requests not fulfilling JWT requirements
/// are blocked with an Http `401` status code. Valid requests
/// are forwarded to the next Http handler.
///
/// # How to integrate
/// ```rust
/// async fn spawn_jwt_server() {
///  use crate::EngineApi;
///  use jsonrpsee::server::ServerBuilder;
///  use reth_primitives::MAINNET;
///  use reth_provider::test_utils::MockEthProvider;
///  use reth_rpc_api::EngineApiServer;
///  use std::{net::SocketAddr, sync::Arc};
///  use tokio::sync::mpsc::unbounded_channel;
///  
///  const AUTH_PORT: u32 = 8551;
///  const AUTH_ADDR: &str = "0.0.0.0";
///  const AUTH_SECRET: &str = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";
///  
///  let (tx, _rx) = unbounded_channel();
///  let _chain_spec = MAINNET.clone();
///  let _client = Arc::new(MockEthProvider::default());
///  
///  // rx side of the channel should be provided
///  // to a consensus engine implementation.
///  // example pseudocode:
///  //
///  // tokio::spawn(EthConsensusEngine::new(
///  //     Arc::clone(&client),
///  //     chain_spec.clone(),
///  //     UnboundedReceiverStream::new(_rx),
///  //     Default::default(),
///  // ));
///  
///  let addr = format!("{AUTH_ADDR}:{AUTH_PORT}");
///  let secret: JwtSecret = AUTH_SECRET.try_into().unwrap();
///  let layer = JwtLayer::new(secret);
///  let middleware = tower::ServiceBuilder::default().layer(layer);
///  
///  let server = ServerBuilder::default()
///      .set_middleware(middleware)
///      .build(addr.parse::<SocketAddr>().unwrap())
///      .await
///      .unwrap();
///  
///  let rpc = EngineApi { engine_tx: tx }.into_rpc();
///  server.start(rpc).unwrap();
/// }
/// ```
#[allow(missing_debug_implementations)]
pub struct JwtLayer(JwtSecret);

impl JwtLayer {
    /// Creates an instance of [`JwtLayer`][crate::engine::JwtLayer].
    pub fn new(secret: JwtSecret) -> Self {
        Self(secret)
    }
}

impl<S> Layer<S> for JwtLayer {
    type Service = JwtService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        JwtService { secret: self.0.clone(), inner }
    }
}

/// This trait is the actual implementation of
/// the middleware. It follows the [`Service`](tower::Service)
/// specification to correctly proxy Http requests
/// to its inner service.
#[allow(missing_debug_implementations)]
pub struct JwtService<S> {
    /// Performs auth validation logics
    secret: JwtSecret,
    /// Recipient of authorized Http requests
    inner: S,
}

impl<ReqBody, ResBody, S> Service<Request<ReqBody>> for JwtService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    ResBody: Default,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future, ResBody>;

    /// If we get polled it means that we dispatched an authorized Http request to the inner layer.
    /// So we just poll the inner layer ourselves.
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    /// This is the entrypoint of the service. We receive an Http request and check the validity of
    /// the authorization header.
    ///
    /// Returns a future that wraps either:
    /// - The inner service future for authorized requests
    /// - An error response in case of authorization errors
    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        match get_bearer(req.headers()) {
            Some(jwt) => match self.secret.validate(jwt) {
                Ok(_) => ResponseFuture::future(self.inner.call(req)),
                Err(e) => {
                    error!(target: "engine::jwt-layer", "Auhorization error: {e:?}");
                    ResponseFuture::invalid_auth()
                }
            },
            None => {
                error!(target: "engine::jwt-layer", "Auhorization header not found");
                ResponseFuture::invalid_auth()
            }
        }
    }
}

#[pin_project]
#[allow(missing_debug_implementations)]
pub struct ResponseFuture<F, B> {
    #[pin]
    kind: Kind<F, B>,
}

impl<F, B> ResponseFuture<F, B>
where
    B: Default,
{
    fn future(future: F) -> Self {
        Self { kind: Kind::Future { future } }
    }

    fn invalid_auth() -> Self {
        let mut res = Response::new(B::default());
        *res.status_mut() = StatusCode::UNAUTHORIZED;
        Self { kind: Kind::Error { response: Some(res) } }
    }
}

#[pin_project(project = KindProj)]
enum Kind<F, B> {
    Future {
        #[pin]
        future: F,
    },
    Error {
        response: Option<Response<B>>,
    },
}

impl<F, B, E> Future for ResponseFuture<F, B>
where
    F: Future<Output = Result<Response<B>, E>>,
    B: Default,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().kind.project() {
            KindProj::Future { future } => future.poll(cx),
            KindProj::Error { response } => {
                let response = response.take().unwrap();
                Poll::Ready(Ok(response))
            }
        }
    }
}

/// This is an utility function that retrieves a bearer
/// token from an authorization Http header.
fn get_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .map(|header| header.to_str())
        .and_then(|result| result.ok())
        .and_then(|auth| {
            let prefix = "Bearer ";
            auth.find(prefix)
                .map(|index| &auth[index + prefix.len()..])
                .map(|token| token.to_owned())
        })
}

#[cfg(test)]
mod tests {
    use crate::engine::jwt_layer::get_bearer;
    use http::{header, HeaderMap};

    #[test]
    fn auth_header_available() {
        let jwt = "foo";
        let bearer = format!("Bearer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers).unwrap();
        assert_eq!(token, jwt);
    }

    #[test]
    fn auth_header_not_available() {
        let headers = HeaderMap::new();
        let token = get_bearer(&headers);
        assert!(matches!(token, None));
    }

    #[test]
    fn auth_header_malformed() {
        let jwt = "foo";
        let bearer = format!("Bea___rer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers);
        assert!(matches!(token, None));
    }
}
