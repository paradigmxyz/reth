use super::AuthValidator;
use http::{Request, Response};
use http_body::Body;
use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};

/// This is an Http middleware layer that acts as an
/// interceptor for `Authorization` headers. Incoming requests are dispatched to
/// an inner [`AuthValidator`]. Invalid requests are blocked and the validator's error response is
/// returned. Valid requests are instead dispatched to the next layer along the chain.
///
/// # How to integrate
/// ```rust
/// async fn build_layered_rpc_server() {
///     use jsonrpsee::server::ServerBuilder;
///     use reth_rpc::{AuthLayer, JwtAuthValidator, JwtSecret};
///     use std::net::SocketAddr;
///
///     const AUTH_PORT: u32 = 8551;
///     const AUTH_ADDR: &str = "0.0.0.0";
///     const AUTH_SECRET: &str =
///         "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";
///
///     let addr = format!("{AUTH_ADDR}:{AUTH_PORT}");
///     let secret = JwtSecret::from_hex(AUTH_SECRET).unwrap();
///     let validator = JwtAuthValidator::new(secret);
///     let layer = AuthLayer::new(validator);
///     let middleware = tower::ServiceBuilder::default().layer(layer);
///
///     let _server = ServerBuilder::default()
///         .set_middleware(middleware)
///         .build(addr.parse::<SocketAddr>().unwrap())
///         .await
///         .unwrap();
/// }
/// ```
#[allow(missing_debug_implementations)]
pub struct AuthLayer<V> {
    validator: V,
}

impl<V> AuthLayer<V>
where
    V: AuthValidator,
    V::ResponseBody: Body,
{
    /// Creates an instance of [`AuthLayer`].
    /// `validator` is a generic trait able to validate requests (see [`AuthValidator`]).
    pub fn new(validator: V) -> Self {
        Self { validator }
    }
}

impl<S, V> Layer<S> for AuthLayer<V>
where
    V: Clone,
{
    type Service = AuthService<S, V>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService { validator: self.validator.clone(), inner }
    }
}

/// This type is the actual implementation of the middleware. It follows the [`Service`]
/// specification to correctly proxy Http requests to its inner service after headers validation.
#[derive(Debug)]
pub struct AuthService<S, V> {
    /// Performs auth validation logics
    validator: V,
    /// Recipient of authorized Http requests
    inner: S,
}

impl<ReqBody, ResBody, S, V> Service<Request<ReqBody>> for AuthService<S, V>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    V: AuthValidator<ResponseBody = ResBody>,
    ReqBody: Body,
    ResBody: Body,
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
    /// - An error Http response in case of authorization errors
    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        match self.validator.validate(req.headers()) {
            Ok(_) => ResponseFuture::future(self.inner.call(req)),
            Err(res) => ResponseFuture::invalid_auth(res),
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
    B: Body,
{
    fn future(future: F) -> Self {
        Self { kind: Kind::Future { future } }
    }

    fn invalid_auth(err_res: Response<B>) -> Self {
        Self { kind: Kind::Error { response: Some(err_res) } }
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
    B: Body,
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

#[cfg(test)]
mod tests {

    use http::{header, Method, Request, StatusCode};
    use hyper::{body, Body};
    use jsonrpsee::{
        server::{RandomStringIdProvider, ServerBuilder, ServerHandle},
        RpcModule,
    };
    use std::{
        net::SocketAddr,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::AuthLayer;
    use crate::{layers::jwt_secret::Claims, JwtAuthValidator, JwtError, JwtSecret};

    const AUTH_PORT: u32 = 8551;
    const AUTH_ADDR: &str = "0.0.0.0";
    const SECRET: &str = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";

    #[tokio::test]
    async fn test_jwt_layer() {
        // We group all tests into one to avoid individual #[tokio::test]
        // to concurrently spawn a server on the same port.
        valid_jwt().await;
        missing_jwt_error().await;
        wrong_jwt_signature_error().await;
        invalid_issuance_timestamp_error().await;
        jwt_decode_error().await;
    }

    async fn valid_jwt() {
        let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
        let secret = JwtSecret::from_hex(SECRET).unwrap(); // Same secret as the server
        let jwt = secret.encode(&claims).unwrap();
        let (status, _) = send_request(Some(jwt)).await;
        assert_eq!(status, StatusCode::OK);
    }

    async fn missing_jwt_error() {
        let (status, body) = send_request(None).await;
        let expected = JwtError::MissingOrInvalidAuthorizationHeader;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, expected.to_string());
    }

    async fn wrong_jwt_signature_error() {
        // This secret is different from the server. This will generate a
        // different signature
        let secret = JwtSecret::random();
        let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
        let jwt = secret.encode(&claims).unwrap();

        let (status, body) = send_request(Some(jwt)).await;
        let expected = JwtError::InvalidSignature;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, expected.to_string());
    }

    async fn invalid_issuance_timestamp_error() {
        let secret = JwtSecret::from_hex(SECRET).unwrap(); // Same secret as the server

        let iat = to_u64(SystemTime::now()) + 1000;
        let claims = Claims { iat, exp: Some(10000000000) };
        let jwt = secret.encode(&claims).unwrap();

        let (status, body) = send_request(Some(jwt)).await;
        let expected = JwtError::InvalidIssuanceTimestamp;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, expected.to_string());
    }

    async fn jwt_decode_error() {
        let jwt = "this jwt has serious encoding problems".to_string();
        let (status, body) = send_request(Some(jwt)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, "JWT decoding error: InvalidToken".to_string());
    }

    async fn send_request(jwt: Option<String>) -> (StatusCode, String) {
        let server = spawn_server().await;
        let client = hyper::Client::new();

        let jwt = jwt.unwrap_or_default();
        let address = format!("http://{AUTH_ADDR}:{AUTH_PORT}");
        let bearer = format!("Bearer {jwt}");
        let body = r#"{"jsonrpc": "2.0", "method": "greet_melkor", "params": [], "id": 1}"#;

        let req = Request::builder()
            .method(Method::POST)
            .header(header::AUTHORIZATION, bearer)
            .header(header::CONTENT_TYPE, "application/json")
            .uri(address)
            .body(Body::from(body))
            .unwrap();

        let res = client.request(req).await.unwrap();
        let status = res.status();
        let body_bytes = body::to_bytes(res.into_body()).await.unwrap();
        let body = String::from_utf8(body_bytes.to_vec()).expect("response was not valid utf-8");

        server.stop().unwrap();
        server.stopped().await;

        (status, body)
    }

    /// Spawn a new RPC server equipped with a JwtLayer auth middleware.
    async fn spawn_server() -> ServerHandle {
        let secret = JwtSecret::from_hex(SECRET).unwrap();
        let addr = format!("{AUTH_ADDR}:{AUTH_PORT}");
        let validator = JwtAuthValidator::new(secret);
        let layer = AuthLayer::new(validator);
        let middleware = tower::ServiceBuilder::default().layer(layer);

        // Create a layered server
        let server = ServerBuilder::default()
            .set_id_provider(RandomStringIdProvider::new(16))
            .set_middleware(middleware)
            .build(addr.parse::<SocketAddr>().unwrap())
            .await
            .unwrap();

        // Create a mock rpc module
        let mut module = RpcModule::new(());
        module.register_method("greet_melkor", |_, _| "You are the dark lord").unwrap();

        server.start(module)
    }

    fn to_u64(time: SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }
}
