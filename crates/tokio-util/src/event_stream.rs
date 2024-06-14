//! Event streams related functionality.

use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio_stream::Stream;
use tracing::warn;

/// Thin wrapper around tokio's `BroadcastStream` to allow skipping broadcast errors.
#[derive(Debug)]
pub struct EventStream<T> {
    inner: tokio_stream::wrappers::BroadcastStream<T>,
}

impl<T> EventStream<T>
where
    T: Clone + Send + 'static,
{
    /// Creates a new `EventStream`.
    pub fn new(receiver: tokio::sync::broadcast::Receiver<T>) -> Self {
        let inner = tokio_stream::wrappers::BroadcastStream::new(receiver);
        Self { inner }
    }
}

impl<T> Stream for EventStream<T>
where
    T: Clone + Send + 'static,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(item))) => return Poll::Ready(Some(item)),
                Poll::Ready(Some(Err(e))) => {
                    warn!("BroadcastStream lagged: {e:?}");
                    continue
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn test_event_stream_yields_items() {
        let (tx, _) = broadcast::channel(16);
        let my_stream = EventStream::new(tx.subscribe());

        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();

        // drop the sender to terminate the stream and allow collect to work.
        drop(tx);

        let items: Vec<i32> = my_stream.collect().await;

        assert_eq!(items, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_event_stream_skips_lag_errors() {
        let (tx, _) = broadcast::channel(2);
        let my_stream = EventStream::new(tx.subscribe());

        let mut _rx2 = tx.subscribe();
        let mut _rx3 = tx.subscribe();

        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();
        tx.send(4).unwrap(); // This will cause lag for the first subscriber

        // drop the sender to terminate the stream and allow collect to work.
        drop(tx);

        // Ensure lag errors are skipped and only valid items are collected
        let items: Vec<i32> = my_stream.collect().await;

        assert_eq!(items, vec![3, 4]);
    }
}
