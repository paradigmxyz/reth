//! [`jsonrpsee`] helper layer for rate limiting certain methods.

use jsonrpsee::{server::middleware::rpc::RpcServiceT, types::Request, MethodResponse};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::{either::Either, sync::PollSemaphore};
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
    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        let method_name = req.method_name();
        if method_name.starts_with("trace_") || method_name.starts_with("debug_") {
            // Sync methods and the params decoding of async methods run inside `call`, so the
            // inner service is only called once the permit is held.
            let inner = self.inner.clone();
            Either::Left(RateLimitingRequestFuture {
                fut: async move { inner.call(req).await },
                guard: Some(self.rate_limiter.inner.call_guard.clone()),
                permit: None,
            })
        } else {
            // if we don't need to rate limit, then there
            // is no need to get a semaphore permit
            Either::Right(RateLimitingRequestFuture {
                fut: self.inner.call(req),
                guard: None,
                permit: None,
            })
        }
    }

    fn batch<'a>(
        &self,
        requests: jsonrpsee::core::middleware::Batch<'a>,
    ) -> impl Future<Output = MethodResponse> + Send + 'a {
        self.inner.batch(requests)
    }

    fn notification<'a>(
        &self,
        n: jsonrpsee::core::middleware::Notification<'a>,
    ) -> impl Future<Output = MethodResponse> + Send + 'a {
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
    use jsonrpsee::{
        core::middleware::{Batch, Notification},
        types::Id,
        ResponsePayload,
    };
    use std::{
        future::poll_fn,
        pin::pin,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tokio_util::sync::CancellationToken;

    /// Counts the calls that reach it and completes them once `gate` is cancelled.
    #[derive(Clone, Default)]
    struct GatedService {
        calls: Arc<AtomicUsize>,
        gate: CancellationToken,
    }

    impl RpcServiceT for GatedService {
        fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let gate = self.gate.clone();
            async move {
                gate.cancelled().await;
                MethodResponse::response(req.id(), ResponsePayload::success(true), usize::MAX)
            }
        }

        fn batch<'a>(&self, _: Batch<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
            std::future::ready(MethodResponse::notification())
        }

        fn notification<'a>(
            &self,
            _: Notification<'a>,
        ) -> impl Future<Output = MethodResponse> + Send + 'a {
            std::future::ready(MethodResponse::notification())
        }
    }

    fn request(method: &'static str) -> Request<'static> {
        Request::borrowed(method, None, Id::Number(0))
    }

    async fn poll_once<F: Future>(mut fut: Pin<&mut F>) -> Poll<F::Output> {
        poll_fn(|cx| Poll::Ready(fut.as_mut().poll(cx))).await
    }

    #[tokio::test]
    async fn limited_call_reaches_inner_service_with_permit() {
        let inner = GatedService::default();
        let service = RpcRequestRateLimiter::new(1).layer(inner.clone());

        // Takes the only permit and waits in the inner service.
        let mut first = pin!(service.call(request("trace_block")));
        assert!(poll_once(first.as_mut()).await.is_pending());
        assert_eq!(inner.calls.load(Ordering::Relaxed), 1);

        // No permit is left, so the inner service must not see this call yet.
        let mut second = pin!(service.call(request("debug_traceBlock")));
        assert!(poll_once(second.as_mut()).await.is_pending());
        assert_eq!(inner.calls.load(Ordering::Relaxed), 1);

        // Other methods are not limited.
        let mut unlimited = pin!(service.call(request("eth_blockNumber")));
        assert!(poll_once(unlimited.as_mut()).await.is_pending());
        assert_eq!(inner.calls.load(Ordering::Relaxed), 2);

        inner.gate.cancel();
        assert!(first.await.is_success());
        assert!(second.await.is_success());
        assert_eq!(inner.calls.load(Ordering::Relaxed), 3);
        assert!(unlimited.await.is_success());
    }
}
