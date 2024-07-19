use tower::layer::Layer;

//use tower::limit::concurrency::future::ResponseFuture;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::PollSemaphore;
//use tower::Service;

use futures_core::ready;
use std::{
    sync::Arc,
    task::{Context, Poll},
};

/// Enforces a limit on the concurrent number of requests the underlying
/// service can handle.
#[derive(Debug)]
pub struct ConcurrencyLimit<T> {
    inner: T,
    semaphore: PollSemaphore,
    /// The currently acquired semaphore permit, if there is sufficient
    /// concurrency to send a new request.
    ///
    /// The permit is acquired in `poll_ready`, and taken in `call` when sending
    /// a new request.
    permit: Option<OwnedSemaphorePermit>,
}

impl<T> ConcurrencyLimit<T> {
    /// Create a new concurrency limiter.
    pub fn new(inner: T, max: usize) -> Self {
        Self::with_semaphore(inner, Arc::new(Semaphore::new(max)))
    }

    /// Create a new concurrency limiter with a provided shared semaphore
    pub fn with_semaphore(inner: T, semaphore: Arc<Semaphore>) -> Self {
        ConcurrencyLimit {
            inner,
            semaphore: PollSemaphore::new(semaphore),
            permit: None,
        }
    }

    /// Get a reference to the inner service
    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner service
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consume `self`, returning the inner service
    pub fn into_inner(self) -> T {
        self.inner
    }
}

/// Enforces a limit on the concurrent number of requests the underlying
/// service can handle.
#[derive(Debug, Clone)]
pub struct ConcurrencyLimitLayer {
    max: usize,
}

impl ConcurrencyLimitLayer {
    /// Create a new concurrency limit layer.
    pub fn new(max: usize) -> Self {
        ConcurrencyLimitLayer { max }
    }
}

impl<S> Layer<S> for ConcurrencyLimitLayer {
    type Service = ConcurrencyLimit<S>;

    fn layer(&self, service: S) -> Self::Service {
        ConcurrencyLimit::new(service, self.max)
    }
}

//use tower::limit::concurrency::ConcurrencyLimit;
//use tower::limit::concurrency::ConcurrencyLimitLayer;
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, RpcServiceBuilder},
    types::Request,
    MethodResponse,
};
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};


impl<'a, S> RpcServiceT<'a> for ConcurrencyLimit<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    //type Future = Pin<Box<dyn Future<Output = MethodResponse> + Send + 'a>>;
    type Future = ResponseFuture<S::Future>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        // tracing::info!("MyMiddleware processed call {}", req.method);
        // let count = self.count.clone();
        // let service = self.service.clone();
        // Box::pin(async move {
        //     let rp = service.call(req).await;
        //     // Modify the state.
        //     count.fetch_add(1, Ordering::Relaxed);
        //     rp
        // })

        // Take the permit
        let permit = self
            .permit
            .take()
            .expect("max requests in-flight; poll_ready must be called first");

        // Call the inner service
        let future = self.inner.call(req);

        ResponseFuture::new(future, permit)
    }
}


#[pin_project::pin_project]
/// Future for the [`ConcurrencyLimit`] service.
///
/// [`ConcurrencyLimit`]: crate::limit::ConcurrencyLimit
#[derive(Debug)]
pub struct ResponseFuture<T> {
    #[pin]
    inner: T,
    // Keep this around so that it is dropped when the future completes
    permit: OwnedSemaphorePermit,
}

impl<T> ResponseFuture<T> {
    pub(crate) fn new(inner: T, permit: OwnedSemaphorePermit) -> ResponseFuture<T> {
        ResponseFuture { inner, permit }
    }
}