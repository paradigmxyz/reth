//! RPC middleware for rate limiting certain methods.

use reth_json_rpc::{MethodResponse, Notification, Request, RpcServiceT};
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

impl<S> RpcServiceT for RpcRequestRateLimitingService<S>
where
    S: RpcServiceT + Send + Sync + Clone + 'static,
{
    fn call(&self, req: Request) -> impl Future<Output = MethodResponse> + Send {
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

    fn notification(&self, n: Notification) -> impl Future<Output = ()> + Send {
        self.inner.notification(n)
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

impl<F: Future> Future for RateLimitingRequestFuture<F> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_json_rpc::{Id, Params};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Records the highest number of calls running at once.
    #[derive(Clone, Default)]
    struct Concurrency {
        running: Arc<AtomicUsize>,
        max: Arc<AtomicUsize>,
    }

    impl RpcServiceT for Concurrency {
        async fn call(&self, req: Request) -> MethodResponse {
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(running, Ordering::SeqCst);
            tokio::task::yield_now().await;
            self.running.fetch_sub(1, Ordering::SeqCst);
            MethodResponse::response(req.id, &(), usize::MAX)
        }
    }

    #[tokio::test]
    async fn rate_limits_batch_entries() {
        let inner = Concurrency::default();
        let service = RpcRequestRateLimiter::new(1).layer(inner.clone());
        let reqs = (0..3)
            .map(|id| Request::new("trace_block", Params::new(None), Id::Number(id)))
            .collect();
        assert_eq!(service.batch(reqs).await.len(), 3);
        assert_eq!(inner.max.load(Ordering::SeqCst), 1);
    }
}
