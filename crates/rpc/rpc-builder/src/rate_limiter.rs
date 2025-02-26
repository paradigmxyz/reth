//! [`jsonrpsee`] helper layer for rate limiting certain methods.

use jsonrpsee::{server::middleware::rpc::RpcServiceT, types::Request, MethodResponse};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::PollSemaphore;
use tower::Layer;

/// Rate limiter for the RPC server.
///
/// Rate limits expensive calls such as debug_ and trace_.
#[derive(Debug, Clone)]
pub struct RpcRequestRateLimiter {
    inner: Arc<RpcRequestRateLimiterInner>,
}

impl RpcRequestRateLimiter {
    /// Create a new rate limit layer with the given number of permits.
    pub fn new(rate_limit: usize) -> Self {
        Self {
            inner: Arc::new(RpcRequestRateLimiterInner {
                call_guard: PollSemaphore::new(Arc::new(Semaphore::new(rate_limit))),
            }),
        }
    }
}

impl<S> Layer<S> for RpcRequestRateLimiter {
    type Service = RpcRequestRateLimitingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RpcRequestRateLimitingService::new(inner, self.clone())
    }
}

/// Rate Limiter for the RPC server
#[derive(Debug, Clone)]
struct RpcRequestRateLimiterInner {
    /// Semaphore to rate limit calls
    call_guard: PollSemaphore,
}

/// A [`RpcServiceT`] middleware that rate limits RPC calls to the server.
#[derive(Debug, Clone)]
pub struct RpcRequestRateLimitingService<S> {
    /// The rate limiter for RPC requests
    rate_limiter: RpcRequestRateLimiter,
    /// The inner service being wrapped
    inner: S,
}

impl<S> RpcRequestRateLimitingService<S> {
    /// Create a new rate limited service.
    pub const fn new(service: S, rate_limiter: RpcRequestRateLimiter) -> Self {
        Self { inner: service, rate_limiter }
    }
}

impl<'a, S> RpcServiceT<'a> for RpcRequestRateLimitingService<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = RateLimitingRequestFuture<S::Future>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        let method_name = req.method_name();
        if method_name.starts_with("trace_") || method_name.starts_with("debug_") {
            RateLimitingRequestFuture {
                fut: self.inner.call(req),
                guard: Some(self.rate_limiter.inner.call_guard.clone()),
                permit: None,
            }
        } else {
            // if we don't need to rate limit, then there
            // is no need to get a semaphore permit
            RateLimitingRequestFuture { fut: self.inner.call(req), guard: None, permit: None }
        }
    }
}

/// Response future.
#[pin_project::pin_project]
pub struct RateLimitingRequestFuture<F> {
    #[pin]
    fut: F,
    guard: Option<PollSemaphore>,
    permit: Option<OwnedSemaphorePermit>,
}

impl<F> std::fmt::Debug for RateLimitingRequestFuture<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RateLimitingRequestFuture")
    }
}

impl<F: Future<Output = MethodResponse>> Future for RateLimitingRequestFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if let Some(guard) = this.guard.as_mut() {
            *this.permit = ready!(guard.poll_acquire(cx));
            *this.guard = None;
        }
        let res = this.fut.poll(cx);
        if res.is_ready() {
            *this.permit = None;
        }
        res
    }
}
