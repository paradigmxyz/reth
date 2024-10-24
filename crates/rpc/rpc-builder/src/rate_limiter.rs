use jsonrpsee::{server::middleware::rpc::RpcServiceT, types::Request, MethodResponse};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::Semaphore;
use tokio_util::sync::PollSemaphore;
use tower::Layer;

/// Rate limiter for the RPC server.
///
/// Rate limits expensive calls such as debug_ and trace_.
#[derive(Debug, Clone)]
pub(crate) struct RpcRequestRateLimiter {
    inner: Arc<RpcRequestRateLimiterInner>,
}

impl RpcRequestRateLimiter {
    pub(crate) fn new(rate_limit: usize) -> Self {
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
///
#[derive(Debug, Clone)]
pub struct RpcRequestRateLimitingService<S> {
    /// The rate limiter for RPC requests
    rate_limiter: RpcRequestRateLimiter,
    /// The inner service being wrapped
    inner: S,
}

impl<S> RpcRequestRateLimitingService<S> {
    pub(crate) fn new(service: S, rate_limiter: RpcRequestRateLimiter) -> Self {
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
            }
        } else {
            // if we don't need to rate limit, then there
            // is no need to get a semaphore permit
            RateLimitingRequestFuture {
                fut: self.inner.call(req),
                guard: None,
            }
        }
    }
}

/// Response future.
#[pin_project::pin_project]
pub struct RateLimitingRequestFuture<F> {
    #[pin]
    fut: F,
    guard: Option<PollSemaphore>,
}

impl<F> std::fmt::Debug for RateLimitingRequestFuture<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RateLimitingRequestFuture")
    }
}

impl<F: Future<Output = MethodResponse>> Future for RateLimitingRequestFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(mut guard) = self.guard.clone() {
            match guard.poll_acquire(cx) {
                Poll::Pending => return Poll::Pending,
                _ => {},
            }
        }
        let this = self.project();
        let res = this.fut.poll(cx);
        res
    }
}
