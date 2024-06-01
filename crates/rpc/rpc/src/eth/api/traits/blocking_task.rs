//! Spawns a blocking task. Should be used for long-lived database reads.

use std::pin::Pin;

use futures::Future;
use reth_primitives::Block;
use tokio::sync::oneshot;

use crate::eth::error::{EthApiError, EthResult};

/// Calls a blocking task on a new blocking thread.
pub trait CallBlocking {
        /// Queues the closure for execution on the blocking thread pool.
        fn spawn_blocking<F, T>(&self, f: F) -> impl Future<Output = EthResult<T>> + Send
        where
        Self: Sized,
            F: FnOnce(Self) -> EthResult<T> + Send + 'static,
            T: Send + 'static;
}
