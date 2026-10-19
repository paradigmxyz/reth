//! Helper types for waiting for the node to exit.

use futures::{future::BoxFuture, FutureExt};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

/// A Future which resolves when the node exits
pub struct NodeExitFuture {
    /// The consensus engine future.
    /// This can be polled to wait for the consensus engine to exit.
    consensus_engine_fut: Option<BoxFuture<'static, eyre::Result<()>>>,
}

impl fmt::Debug for NodeExitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeExitFuture").field("consensus_engine_fut", &"...").finish()
    }
}

impl NodeExitFuture {
    /// Create a new `NodeExitFuture`.
    pub fn new<F>(consensus_engine_fut: F) -> Self
    where
        F: Future<Output = eyre::Result<()>> + 'static + Send,
    {
        Self { consensus_engine_fut: Some(Box::pin(consensus_engine_fut)) }
    }
}

impl Future for NodeExitFuture {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(rx) = this.consensus_engine_fut.as_mut() {
            match ready!(rx.poll_unpin(cx)) {
                Ok(_) => {
                    this.consensus_engine_fut.take();
                    Poll::Ready(Ok(()))
                }
                Err(err) => Poll::Ready(Err(err)),
            }
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_exit_future() {
        let fut = async { Ok(()) };

        let node_exit_future = NodeExitFuture::new(fut);

        let res = node_exit_future.await;

        assert!(res.is_ok());
    }
}
