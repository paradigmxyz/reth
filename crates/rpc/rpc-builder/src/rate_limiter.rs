use jsonrpsee::{server::middleware::rpc::RpcServiceT, types::Request, MethodResponse};
use reth_tasks::pool::BlockingTaskGuard;
use tokio::sync::OwnedSemaphorePermit;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tower::Layer;

/// Rate limiter for the RPC server.
///
/// Rate limits expensive calls such as debug_ and trace_.
#[derive(Debug, Clone)]
pub(crate) struct RpcRequestRateLimiter {
    inner: Arc<RpcRequestRateLimiterInner>,
}

impl RpcRequestRateLimiter {
    pub(crate) fn new(handle: tokio::runtime::Handle, rate_limit: usize) -> Self {
        Self {
            inner: Arc::new(RpcRequestRateLimiterInner {
                handle,
                call_guard: BlockingTaskGuard::new(rate_limit),
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
#[derive(Debug)]
struct RpcRequestRateLimiterInner {
    call_guard: BlockingTaskGuard,
    handle: tokio::runtime::Handle,
}

/// A [`RpcServiceT`] middleware that rate limits RPC calls for the server.
///
#[derive(Clone, Debug)]
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
            let guard = self.rate_limiter.inner.handle.block_on(async {
                self.rate_limiter.inner.call_guard.clone().acquire_owned().await.unwrap()
            });
            RateLimitingRequestFuture {
                fut: self.inner.call(req),
                semaphore_guard: Some(guard),
            }
        } else {
            // if we don't need to rate limit, then there
            // is no need to get a semaphore permit
            RateLimitingRequestFuture {
                fut: self.inner.call(req),
                semaphore_guard: None,
            }
        }
    }
}

/// Response future.
#[pin_project::pin_project]
pub struct RateLimitingRequestFuture<F> {
    #[pin]
    fut: F,
    semaphore_guard: Option<OwnedSemaphorePermit>,
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
        let res = this.fut.poll(cx);
        res
    }
}
