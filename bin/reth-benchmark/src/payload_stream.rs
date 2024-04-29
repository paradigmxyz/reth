use alloy_provider::RootProvider;
use futures::{FutureExt, Stream, StreamExt};
use std::{
    ops::RangeInclusive,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{sleep, Instant, Sleep};

/// Whether or not the benchmark should run as a continuous stream of payloads.
///
/// TODO: build two structs that impl Stream, which return payloads from the local db and from the
/// rpc api.
///
/// impl rpc one first - the payload needs to be ready instantly
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BenchmarkMode {
    /// Run the benchmark as a continuous stream of payloads, until the benchmark is interrupted.
    Continuous,
    /// Run the benchmark for a specific range of blocks.
    Range(RangeInclusive<u64>),
}

/// This implements a stream of blocks for the benchmark mode
#[derive(Debug)]
pub(crate) struct BlockStream<T> {
    /// The mode of the benchmark
    mode: BenchmarkMode,
    /// The provider to be used to fetch blocks
    provider: RootProvider<T>,
}

impl<T> BlockStream<T> {
    /// Create a new `BlockStream` with the given mode and provider.
    ///
    /// Also returns a signal that can be used to cancel the stream.
    ///
    /// # Example
    /// ```no_run
    /// use alloy_provider::ProviderBuilder;
    /// use futures::stream::iter;
    /// use reth_benchmark::BlockStream;
    ///
    /// let block_provider = ProviderBuilder::new().on_http("http://localhost:8551".parse().unwrap());
    /// let (block_stream, cancel) = BlockStream::new(BenchmarkMode::Continuous, block_provider);
    ///
    /// // take some blocks
    /// let blocks = block_stream.take(10).collect().await;
    ///
    /// // cancel the stream (we would be continuous)
    /// cancel.send(()).unwrap();
    ///
    /// // the stream should finish
    /// let next = block_stream.next().await;
    /// assert_eq!(next, None);
    /// ```
    pub(crate) fn new(mode: BenchmarkMode, provider: RootProvider<T>) -> Self {
        Self { mode, provider }
    }
}

/// This is the output stream, which will emit payloads at a given interval.
#[derive(Debug)]
pub(crate) struct PayloadStream<S>
where
    S: Stream + Unpin,
{
    /// The interval between blocks
    block_timer: Pin<Box<Sleep>>,
    /// The stream of payloads.
    stream: S,
    /// Block time
    block_time: Duration,
    /// item ready to be sent
    item: Option<S::Item>,
    /// whether or not the stream is done
    done: bool,
    // TODO: I must be dumb, this all seems way too complicated for what it is
    // I'm over engineering this for sure
}

impl<S, T> PayloadStream<S>
where
    S: Stream<Item = T> + Unpin,
{
    /// Create a new `PayloadStream` with the given block timer and stream.
    pub(crate) fn new(block_time: Duration, stream: S) -> Self {
        Self {
            block_time,
            stream,
            block_timer: Box::pin(sleep(block_time)),
            item: None,
            done: false,
        }
    }
}

impl<S, T> Stream for PayloadStream<S>
where
    S: Stream<Item = T> + Unpin,
    T: Unpin,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(item) = this.item.take() {
            match this.block_timer.poll_unpin(cx) {
                Poll::Ready(()) => {
                    this.block_timer.as_mut().reset(Instant::now() + this.block_time);
                    this.item = None;
                    return Poll::Ready(Some(item));
                }
                Poll::Pending => {
                    this.item = Some(item);
                    return Poll::Pending;
                }
            }
        } else {
            match this.stream.poll_next_unpin(cx) {
                Poll::Ready(Some(item)) => {
                    this.item = Some(item);
                }
                Poll::Ready(None) => {
                    this.done = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {}
            }
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::iter;
    use tokio::{select, time::interval};

    #[tokio::test]
    async fn test_payload_stream_basic() {
        reth_tracing::init_test_tracing();
        let block_timer = std::time::Duration::from_secs(1);
        let stream = iter(vec![1, 2, 3]);
        let mut payload_stream = PayloadStream::new(block_timer, stream);

        // select on 600ms interval and next item in stream
        let mut interval = interval(std::time::Duration::from_millis(600));

        // also tick the first interval because it returns immediately
        interval.tick().await;

        // make sure the sleep is first
        select! {
            _ = interval.tick() => {
                // this is expected
                // 600ms
            }
            item = payload_stream.next() => {
                // this should never happen
                panic!("expected sleep to complete first, got {:?}", item);
            }
        }

        // make sure the item is first
        select! {
            _ = interval.tick() => {
                // this should never happen
                // 1000ms
                panic!("expected payload stream to complete first, instead interval ticked first");
            }
            item = payload_stream.next() => {
                assert_eq!(item, Some(1));
            }
        }

        // make sure the next tick is first
        select! {
            _ = interval.tick() => {
                // this is expected
                // 1200ms
            }
            item = payload_stream.next() => {
                // this should never happen
                panic!("expected sleep to complete first, got {:?}", item);
            }
        }

        // we have another tick before the next item
        select! {
            _ = interval.tick() => {
                // this is expected
                // 1800ms
            }
            item = payload_stream.next() => {
                // this should never happen
                panic!("expected sleep to complete first, got {:?}", item);
            }
        }

        // make sure the item is first
        select! {
            _ = interval.tick() => {
                // this should never happen
                // 2000ms
                panic!("expected payload stream to complete first, instead interval ticked first");
            }
            item = payload_stream.next() => {
                assert_eq!(item, Some(2));
            }
        }

        // we have another tick before the next item
        select! {
            _ = interval.tick() => {
                // this is expected
                // 2400ms
            }
            item = payload_stream.next() => {
                // this should never happen
                panic!("expected sleep to complete first, got {:?}", item);
            }
        }
    }
}
