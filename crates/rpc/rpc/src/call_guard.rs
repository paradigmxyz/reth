use std::sync::Arc;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

/// RPC Tracing call guard semaphore.
///
/// This is used to restrict the number of concurrent RPC requests to tracing methods like
/// `debug_traceTransaction` because they can consume a lot of memory.
#[derive(Clone, Debug)]
pub struct TracingCallGuard(Arc<Semaphore>);

impl TracingCallGuard {
    /// Create a new `TracingCallGuard` with the given maximum number of tracing calls in parallel.
    pub fn new(max_tracing_requests: usize) -> Self {
        Self(Arc::new(Semaphore::new(max_tracing_requests)))
    }

    /// See also [Semaphore::acquire_owned]
    pub async fn acquire_owned(self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.0.acquire_owned().await
    }
}
