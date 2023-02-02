use http::{HeaderMap, Request, Response, StatusCode};

use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};

use crate::JwtSecret;

/// Middleware struct that provides JWT bearer
/// token check for incoming Http requests
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
pub struct JwtService<S> {
    secret: JwtSecret,
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

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        match self.get_bearer(req.headers()) {
            Some(jwt) => match self.secret.validate(jwt) {
                Ok(_) => ResponseFuture::future(self.inner.call(req)),
                Err(_) => {
                    // TODO: Log
                    ResponseFuture::invalid_auth()
                }
            },
            None => {
                // TODO: Log
                ResponseFuture::invalid_auth()
            }
        }
    }
}

impl<S> JwtService<S> {
    // TODO: Add full coverage
    fn get_bearer(&self, headers: &HeaderMap) -> Option<String> {
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
}

#[pin_project]
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
