use crate::{Claims, JwtSecret};
use http::HeaderValue;
use hyper::{header::AUTHORIZATION, service::Service};
use std::{
    task::{Context, Poll},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::Layer;

/// A layer that adds a new JWT token to every request using AuthClientService.
#[derive(Debug)]
pub struct AuthClientLayer {
    secret: JwtSecret,
}

impl AuthClientLayer {
    /// Create a new AuthClientLayer with the given `secret`.
    pub fn new(secret: JwtSecret) -> Self {
        Self { secret }
    }
}

impl<S> Layer<S> for AuthClientLayer {
    type Service = AuthClientService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthClientService::new(self.secret.clone(), inner)
    }
}

/// Automatically authenticates every client request with the given `secret`.
#[derive(Debug, Clone)]
pub struct AuthClientService<S> {
    secret: JwtSecret,
    inner: S,
}

impl<S> AuthClientService<S> {
    fn new(secret: JwtSecret, inner: S) -> Self {
        Self { secret, inner }
    }
}

impl<S, B> Service<hyper::Request<B>> for AuthClientService<S>
where
    S: Service<hyper::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: hyper::Request<B>) -> Self::Future {
        request.headers_mut().insert(AUTHORIZATION, secret_to_bearer_header(&self.secret));
        self.inner.call(request)
    }
}

/// Helper function to convert a secret into a Bearer auth header value with claims according to
/// <https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md#jwt-claims>.
/// The token is valid for 60 seconds.
pub fn secret_to_bearer_header(secret: &JwtSecret) -> HeaderValue {
    format!(
        "Bearer {}",
        secret
            .encode(&Claims {
                iat: (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() +
                    Duration::from_secs(60))
                .as_secs(),
                exp: None,
            })
            .unwrap()
    )
    .parse()
    .unwrap()
}
