use futures_util::{stream::Fuse, StreamExt};
use reth_primitives::TxHash;
use reth_transaction_pool::TransactionPool;
use std::{
    future::Future,
    pin::{pin, Pin},
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Interval;
use tokio_stream::{wrappers::ReceiverStream, Stream};

/// A mining mode for the local dev engine.
///
/// This can either be [`Auto`] which will
/// build a payload as soon as it has transactions
/// in the pool or a [`Interval`] which will build
/// a payload at a predefined interval
#[derive(Debug)]
pub enum MiningMode {
    Auto(Fuse<ReceiverStream<TxHash>>),
    Interval(Interval),
}

impl MiningMode {
    /// Constructor for an [`MiningMode::Auto`]
    pub fn instant<Pool: TransactionPool>(pool: Pool) -> Self {
        let rx = pool.pending_transactions_listener();
        Self::Auto(ReceiverStream::new(rx).fuse())
    }

    /// Constructor for an [`MiningMode::Interval`]
    pub fn interval(duration: Duration) -> Self {
        let start = tokio::time::Instant::now() + duration;
        Self::Interval(tokio::time::interval_at(start, duration))
    }
}

impl Future for MiningMode {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match this {
            Self::Auto(rx) => {
                // drain all transactions notifications
                if let Poll::Ready(Some(_)) = pin!(rx).poll_next(cx) {
                    return Poll::Ready(())
                }
                Poll::Pending
            }
            Self::Interval(interval) => {
                if interval.poll_tick(cx).is_ready() {
                    return Poll::Ready(())
                }
                Poll::Pending
            }
        }
    }
}
