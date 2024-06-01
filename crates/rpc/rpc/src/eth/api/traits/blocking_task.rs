//! Spawns a blocking task. Should be used for long-lived database reads.

use futures::Future;

use crate::eth::error::EthResult;

/// Executes code on a blocking thread.
pub trait SpawnBlocking {
    /// Executes closure on a blocking thread.
    fn spawn_blocking<F, T>(&self, f: F) -> impl Future<Output = EthResult<T>> + Send
    where
        Self: Sized,
        F: FnOnce(Self) -> EthResult<T> + Send + 'static,
        T: Send + 'static;
}
