use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone, Debug)]
/// RPC Tracing call guard semaphore
pub struct TracingCallGuard(Arc<Semaphore>);

impl TracingCallGuard {
    /// Create a new `TracingCallGuard` with the given maximum number of tracing calls in parallel.
    pub fn new(max_tracing_requests: usize) -> Self {
        Self(Arc::new(Semaphore::new(max_tracing_requests)))
    }
}
