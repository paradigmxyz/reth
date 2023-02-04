use http::{Request, Response};
use http_body::Body;
use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};

use super::AuthValidator;

/// This is an Http middleware layer that acts as an
/// interceptor for `Authorization` headers. Incoming requests are dispatched to
/// an inner [`AuthValidator`]. Invalid requests are blocked and the validator's error response is
/// returned. Valid requests are instead dispatched to the next layer along the chain.
///
/// # How to integrate
/// ```rust
/// async fn build_layered_rpc_server() {
///    use jsonrpsee::server::ServerBuilder;
///    use reth_rpc::{AuthLayer, JwtAuthValidator, JwtSecret};
///    use std::net::SocketAddr;
///
///    const AUTH_PORT: u32 = 8551;
///    const AUTH_ADDR: &str = "0.0.0.0";
///    const AUTH_SECRET: &str = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";
///
///    let addr = format!("{AUTH_ADDR}:{AUTH_PORT}");
///    let secret = JwtSecret::from_hex(AUTH_SECRET).unwrap();
///    let validator = JwtAuthValidator::new(secret);
///    let layer = AuthLayer::new(validator);
///    let middleware = tower::ServiceBuilder::default().layer(layer);
///
///    let _server = ServerBuilder::default()
///        .set_middleware(middleware)
///        .build(addr.parse::<SocketAddr>().unwrap())
///        .await
///        .unwrap();
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
    /// Creates an instance of [`AuthLayer`][crate::layers::AuthLayer].
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

/// This type is the actual implementation of
/// the middleware. It follows the [`Service`](tower::Service)
/// specification to correctly proxy Http requests
/// to its inner service after headers validation.
#[allow(missing_debug_implementations)]
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

    use http::{header, HeaderMap};
    use jsonrpsee::{
        core::{client::ClientT, Error},
        http_client::{HttpClient, HttpClientBuilder},
        rpc_params,
        server::{RandomStringIdProvider, ServerBuilder, ServerHandle},
        RpcModule,
    };
    use std::{
        net::SocketAddr,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{layers::jwt_secret::Claims, JwtAuthValidator, JwtSecret};

    use super::AuthLayer;

    const AUTH_PORT: u32 = 8551;
    const AUTH_ADDR: &str = "0.0.0.0";

    #[tokio::test]
    async fn test_jwt_layer() {
        // We group all tests into one to avoid individual #[tokio::test]
        // to concurrently spawn a server on the same port.
        valid_jwt().await;
        missing_jwt_error().await;
        wrong_jwt_signature_error().await;
    }

    async fn valid_jwt() {
        let claims = Claims { iat: to_u64(SystemTime::now()), exp: 10000000000 };
        let secret = JwtSecret::random();
        let jwt = secret.encode(&claims).unwrap();

        let headers = auth_header(Some(jwt));
        let client = build_client(headers);
        let server = spawn_server(secret).await;

        let result: String = client.request("greet_melkor", rpc_params!()).await.unwrap();

        let expected = "You are the dark lord".to_string();
        assert_eq!(result, expected);

        server.stop().unwrap();
        server.stopped().await;
    }

    async fn missing_jwt_error() {
        let empty_headers = auth_header(None);
        let client = build_client(empty_headers);
        let secret = JwtSecret::random();
        let server = spawn_server(secret).await;

        let result: Result<String, jsonrpsee::core::Error> =
            client.request("greet_melkor", rpc_params!()).await;

        assert!(matches!(result, Err(Error::Transport(_))));

        server.stop().unwrap();
        server.stopped().await;
    }

    async fn wrong_jwt_signature_error() {
        // We sign JWT claims with a secret different from the server's secret
        let claims = Claims { iat: to_u64(SystemTime::now()), exp: 10000000000 };
        let secret_1 = JwtSecret::random();
        let jwt = secret_1.encode(&claims).unwrap();

        let headers = auth_header(Some(jwt));
        let client = build_client(headers);

        // We spawn a server with a different secret
        let secret_2 = JwtSecret::random();
        let server = spawn_server(secret_2).await;

        let result: Result<String, jsonrpsee::core::Error> =
            client.request("greet_melkor", rpc_params!()).await;

        assert!(matches!(result, Err(Error::Transport(_))));

        server.stop().unwrap();
        server.stopped().await;
    }

    fn build_client(headers: HeaderMap) -> HttpClient {
        let address = format!("http://{AUTH_ADDR}:{AUTH_PORT}");
        HttpClientBuilder::default().set_headers(headers).build(&address).unwrap()
    }

    /// Spawn a new RPC server equipped with a JwtLayer auth middleware.
    /// `secret` is the JWT secret provided to the middleware.
    async fn spawn_server(secret: JwtSecret) -> ServerHandle {
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
        module.register_method("greet_melkor", |_, _| Ok("You are the dark lord")).unwrap();

        let handle = server.start(module).unwrap();

        handle
    }

    fn auth_header(jwt: Option<String>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(jwt) = jwt {
            let bearer = format!("Bearer {jwt}");
            headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        }
        headers
    }

    fn to_u64(time: SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }
}
