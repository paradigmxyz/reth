//! Helper types for waiting for the node to exit.

use futures::FutureExt;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

/// A Future which resolves when the node exits
#[allow(missing_debug_implementations)]
pub struct NodeExitFuture {
    /// The consensus engine future.
    /// This can be polled to wait for the consensus engine to exit.
    consensus_engine_fut: Option<Pin<Box<dyn Future<Output = eyre::Result<()>>>>>,

    /// Flag indicating whether the node should be terminated after the pipeline sync.
    terminate: bool,
}

impl NodeExitFuture {
    /// Create a new `NodeExitFuture`.
    pub const fn new(
        consensus_engine_fut: Pin<Box<dyn Future<Output = eyre::Result<()>>>>,
        terminate: bool,
    ) -> Self {
        Self { consensus_engine_fut: Some(consensus_engine_fut), terminate }
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
                    if this.terminate {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Pending
                    }
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
    use std::future::poll_fn;

    #[tokio::test]
    async fn test_node_exit_future_terminate_true() {
        let fut = Box::pin(async { Ok(()) });

        let node_exit_future = NodeExitFuture::new(fut, true);

        let res = node_exit_future.await;

        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_node_exit_future_terminate_false() {
        let fut = Box::pin(async { Ok(()) });

        let mut node_exit_future = NodeExitFuture::new(fut, false);
        poll_fn(|cx| {
            assert!(node_exit_future.poll_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
}
