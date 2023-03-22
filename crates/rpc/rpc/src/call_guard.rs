use std::sync::Arc;
use tokio::sync::Semaphore;

/// RPC Tracing call guard semaphore
pub struct TracingCallGuard(Arc<Semaphore>);

impl TracingCallGuard {
    pub fn new(semaphore: Arc<Semaphore>) -> Self {
        Self(semaphore)
    }
}
