//! Helper types for waiting for the node to exit.

use futures::FutureExt;
use reth_beacon_consensus::BeaconConsensusEngineError;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;

/// A Future which resolves when the node exits
#[derive(Debug)]
pub struct NodeExitFuture {
    /// The receiver half of the channel for the consensus engine.
    /// This can be used to wait for the consensus engine to exit.
    consensus_engine_rx: Option<oneshot::Receiver<Result<(), BeaconConsensusEngineError>>>,

    /// Flag indicating whether the node should be terminated after the pipeline sync.
    terminate: bool,
}

impl NodeExitFuture {
    /// Create a new `NodeExitFuture`.
    pub fn new(
        consensus_engine_rx: oneshot::Receiver<Result<(), BeaconConsensusEngineError>>,
        terminate: bool,
    ) -> Self {
        Self { consensus_engine_rx: Some(consensus_engine_rx), terminate }
    }
}

impl Future for NodeExitFuture {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(rx) = this.consensus_engine_rx.as_mut() {
            match ready!(rx.poll_unpin(cx)) {
                Ok(res) => {
                    this.consensus_engine_rx.take();
                    res?;
                    if this.terminate {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Pending
                    }
                }
                Err(err) => Poll::Ready(Err(err.into())),
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
        let (tx, rx) = oneshot::channel::<Result<(), BeaconConsensusEngineError>>();

        let _ = tx.send(Ok(()));

        let node_exit_future = NodeExitFuture::new(rx, true);

        let res = node_exit_future.await;

        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_node_exit_future_terminate_false() {
        let (tx, rx) = oneshot::channel::<Result<(), BeaconConsensusEngineError>>();

        let _ = tx.send(Ok(()));

        let mut node_exit_future = NodeExitFuture::new(rx, false);
        poll_fn(|cx| {
            assert!(node_exit_future.poll_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
}
