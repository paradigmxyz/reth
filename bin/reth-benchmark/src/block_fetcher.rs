use crate::benchmark_mode::BenchmarkMode;
use alloy_provider::{Provider, RootProvider};
use alloy_transport::{Transport, TransportResult};
use futures::{future::BoxFuture, Future, FutureExt, Stream};
use reth_rpc_types::{Block, BlockNumberOrTag};
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// This implements a stream of blocks for the benchmark mode
pub(crate) struct BlockFetcher<'a, T> {
    /// The mode of the benchmark
    mode: BenchmarkMode,
    /// The provider to be used to fetch blocks
    provider: &'a RootProvider<T>,
    /// The sending channel for blocks
    block_sender: Sender<TransportResult<Option<Block>>>,
    /// The current block future in flight
    current_block: BoxFuture<'a, TransportResult<Option<Block>>>,
}

impl<'a, T> BlockFetcher<'a, T>
where
    T: Transport + Clone,
{
    /// Create a new `BlockStream` with the given mode and provider.
    ///
    /// # Example
    /// ```no_run
    /// use alloy_provider::{Provider, RootProvider};
    /// use alloy_transport::HttpTransport;
    /// use futures::FutureExt;
    /// use reth_benchmark::{BenchmarkMode, BlockFetcher};
    /// use reth_rpc_types::Block;
    /// use tokio::sync::mpsc::Receiver;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let provider = RootProvider::new(HttpTransport::new("http://localhost:8545").unwrap());
    ///     let (block_fetcher, block_receiver) =
    ///         BlockFetcher::new(BenchmarkMode::Continuous, &provider, 10).unwrap();
    ///
    ///     let _handle = tokio::spawn(block_fetcher);
    ///     while let Some(block) = block_receiver.recv().await {
    ///         println!("{:?}", block);
    ///     }
    /// }
    /// ```
    #[allow(clippy::type_complexity)]
    pub(crate) fn new(
        mut mode: BenchmarkMode,
        provider: &'a RootProvider<T>,
        buffer_size: usize,
    ) -> Result<(Self, Receiver<TransportResult<Option<Block>>>), BlockFetcherError> {
        let (block_sender, block_receiver) = channel(buffer_size);
        let current_block = match mode {
            BenchmarkMode::Continuous => {
                // fetch Latest block
                provider.get_block_by_number(BlockNumberOrTag::Latest, true)
            }
            BenchmarkMode::Range(ref mut range) => {
                match range.next() {
                    Some(block_number) => {
                        // fetch first block in range
                        provider.get_block_by_number(block_number.into(), true)
                    }
                    None => {
                        // return an error
                        return Err(BlockFetcherError::RangeEmpty);
                    }
                }
            }
        };
        let block_stream = Self { mode, provider, block_sender, current_block };
        Ok((block_stream, block_receiver))
    }
}

/// All error variants for the block stream
#[derive(Debug, thiserror::Error)]
pub enum BlockFetcherError {
    /// The range is empty
    #[error("The range is empty")]
    RangeEmpty,
}

impl<'a, T> Future for BlockFetcher<'a, T>
where
    T: Transport + Clone,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // if there is capacity in the channel and we are not done, poll the next block
        match this.block_sender.try_reserve() {
            Ok(permit) => {
                // we have capacity, poll the next block
                match ready!(this.current_block.poll_unpin(cx)) {
                    Ok(Some(block)) => {
                        let next_block = match block.header.number {
                            Some(number) => {
                                // fetch next block
                                number + 1
                            }
                            None => {
                                // this should never happen
                                // TODO: log or return error, we should probably not return the
                                // block here
                                return Poll::Ready(());
                            }
                        };

                        // send the block
                        permit.send(Ok(Some(block)));

                        // fetch the next block
                        this.current_block = match this.mode {
                            BenchmarkMode::Continuous => {
                                this.provider.get_block_by_number(next_block.into(), true)
                            }
                            BenchmarkMode::Range(ref mut range) => {
                                match range.next() {
                                    Some(block_number) => {
                                        // fetch next block in range
                                        this.provider.get_block_by_number(block_number.into(), true)
                                    }
                                    None => {
                                        // just finish the future as there is nothing left to do,
                                        // there are no more blocks to fetch and no more blocks to
                                        // send
                                        return Poll::Ready(());
                                    }
                                }
                            }
                        };
                    }
                    Ok(None) => {
                        // this means the stream could not get the next block
                        // TODO: what should we do here?
                        println!("Block stream returned None");
                        permit.send(Ok(None));
                        // // fetch the next block
                        // this.current_block = match this.mode {
                        //     BenchmarkMode::Continuous => {
                        //         this.provider.get_block_by_number(next_block.into(), true)
                        //     }
                        //     BenchmarkMode::Range(ref mut range) => {
                        //         match range.next() {
                        //             Some(block_number) => {
                        //                 // fetch next block in range
                        //                 this.provider.get_block_by_number(block_number.into(),
                        // true)             }
                        //             None => {
                        //                 // just finish the future as there is nothing left to do,
                        //                 // there are no more blocks to fetch and no more blocks
                        // to                 // send
                        //                 return Poll::Ready(());
                        //             }
                        //         }
                        //     }
                        // };
                        return Poll::Ready(());
                    }
                    Err(e) => {
                        // send the error
                        permit.send(Err(e));
                        return Poll::Ready(());
                    }
                }
            }
            Err(_) => {
                // channel is full
            }
        }

        // nothing to do
        Poll::Pending
    }
}

/// Builds a stream out of the block fetcher and its receiver, closing the stream when the task is
/// done and nothing is left in the stream.
pub struct BlockStream<'a, T> {
    /// The block fetcher, if this is None then its future has finished
    block_fetcher: Option<BlockFetcher<'a, T>>,
    /// The receiver for the block fetcher
    block_receiver: Receiver<TransportResult<Option<Block>>>,
}

impl<'a, T> BlockStream<'a, T>
where
    T: Transport + Clone,
{
    /// Create a new `BlockStream` with the given mode and provider.
    ///
    /// # Example
    /// ```no_run
    /// use alloy_provider::{Provider, RootProvider};
    /// use alloy_transport::HttpTransport;
    /// use futures::StreamExt;
    /// use reth_benchmark::{BenchmarkMode, BlockStream};
    /// use reth_rpc_types::Block;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let provider = RootProvider::new(HttpTransport::new("http://localhost:8545").unwrap());
    ///     let block_stream = BlockStream::new(BenchmarkMode::Continuous, &provider, 10).unwrap();
    ///     while let Some(block) = block_stream.next().await {
    ///         println!("{:?}", block);
    ///     }
    /// }
    /// ```
    pub(crate) fn new(
        mode: BenchmarkMode,
        provider: &'a RootProvider<T>,
        buffer_size: usize,
    ) -> Result<Self, BlockFetcherError> {
        let (block_fetcher, block_receiver) = BlockFetcher::new(mode, provider, buffer_size)?;
        Ok(Self { block_fetcher: Some(block_fetcher), block_receiver })
    }
}

impl<'a, T> Stream for BlockStream<'a, T>
where
    T: Transport + Clone,
{
    type Item = TransportResult<Option<Block>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut();

        if let Some(block_fetcher) = this.block_fetcher.as_mut() {
            // poll the block fetcher
            if block_fetcher.poll_unpin(cx) == Poll::Ready(()) {
                // block fetcher is done
                this.block_fetcher = None;
            }
        }

        // now we poll the receiver for an item
        match this.block_receiver.poll_recv(cx) {
            Poll::Ready(None) => {
                // receiver is done
                Poll::Ready(None)
            }
            Poll::Ready(Some(block)) => {
                // receiver has a block
                Poll::Ready(Some(block))
            }
            Poll::Pending => {
                // receiver is pending
                Poll::Pending
            }
        }
    }
}
