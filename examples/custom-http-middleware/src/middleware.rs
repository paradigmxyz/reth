//! A Tower service used as a HTTP middleware for all requests

use std::task::Poll;

use jsonrpsee::{
    core::BoxError,
    server::{HttpRequest, HttpResponse},
};
use tower::{Layer, Service};
use tracing::info;

#[derive(Debug, Default, Clone)]
pub struct CustomLayer;

impl<S> Layer<S> for CustomLayer {
    type Service = CustomService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CustomService::new(inner)
    }
}

#[derive(Debug, Clone)]
pub struct CustomService<S> {
    inner: S,
}

impl<S> CustomService<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Service<HttpRequest> for CustomService<S>
where
    S: Service<HttpRequest, Response = HttpResponse> + Send + Sync + Clone + 'static,
    S::Response: 'static,
    S::Error: Into<BoxError> + 'static,
    S::Future: Send + 'static,
{
    type Error = S::Error;
    type Future = S::Future;
    type Response = HttpResponse;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        let headers = req.headers();
        for (key, value) in headers.iter() {
            info!(target: "reth::custom_http_middleware", "Header: {} = {:?}", key, value);
        }

        self.inner.call(req)
    }
}
