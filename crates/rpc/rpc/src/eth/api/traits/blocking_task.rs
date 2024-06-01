//! Spawns a blocking task. Should be used for long-lived database reads.

use std::pin::Pin;

use futures::Future;
use tokio::sync::oneshot;

use crate::eth::error::{EthApiError, EthResult};

/// Calls a blocking task on a new blocking thread.
pub trait CallBlocking {
    /// Spawns a blocking task.
    fn spawn_blocking<F>(&self, f: Pin<Box<F>>)
    where
        F: Future<Output = ()> + Send + 'static;
    /// Executes the future on a new blocking task.
    ///
    /// This accepts a closure that creates a new future using a clone of this type and spawns the
    /// future onto a new task that is allowed to block.
    ///
    /// Note: This is expected for futures that are dominated by blocking IO operations.
    fn on_blocking_task<C, F, R>(&self, c: C) -> impl Future<Output = EthResult<R>>
    where
        Self: Clone + Send + 'static,
        C: FnOnce(Self) -> F,
        F: Future<Output = EthResult<R>> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        let f = c(this);
        self.spawn_blocking(Box::pin(async move {
            let res = f.await;
            let _ = tx.send(res);
        }));

        async move { rx.await.map_err(|_| EthApiError::InternalEthError)? }
    }
}
