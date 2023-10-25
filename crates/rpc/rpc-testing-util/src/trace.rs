//! Helpers for testing trace calls.
use futures::{Stream, StreamExt};
use jsonrpsee::core::Error as RpcError;
use reth_primitives::{BlockId, TxHash};
use reth_rpc_api::clients::TraceApiClient;
use reth_rpc_types::trace::parity::{LocalizedTransactionTrace, TraceResults, TraceType};
use std::{
    collections::HashSet,
    pin::Pin,
    task::{Context, Poll},
};
/// A result type for the `trace_block` method that also captures the requested block.
pub type TraceBlockResult = Result<(Vec<LocalizedTransactionTrace>, BlockId), (RpcError, BlockId)>;
/// Type alias representing the result of replaying a transaction.

pub type ReplayTransactionResult = Result<(TraceResults, TxHash), (RpcError, TxHash)>;

/// An extension trait for the Trace API.
#[async_trait::async_trait]
pub trait TraceApiExt {
    /// The provider type that is used to make the requests.
    type Provider;

    /// Returns a new stream that yields the traces for the given blocks.
    ///
    /// See also [StreamExt::buffered].
    fn trace_block_buffered<I, B>(&self, params: I, n: usize) -> TraceBlockStream<'_>
    where
        I: IntoIterator<Item = B>,
        B: Into<BlockId>;

    /// Returns a new stream that yields the traces for the given blocks.
    ///
    /// See also [StreamExt::buffer_unordered].
    fn trace_block_buffered_unordered<I, B>(&self, params: I, n: usize) -> TraceBlockStream<'_>
    where
        I: IntoIterator<Item = B>,
        B: Into<BlockId>;

    /// Returns a new stream that replays the transactions for the given transaction hashes.
    ///
    /// This returns all results in order.
    fn replay_transactions<I>(
        &self,
        tx_hashes: I,
        trace_types: HashSet<TraceType>,
    ) -> ReplayTransactionStream<'_>
    where
        I: IntoIterator<Item = TxHash>;
}

/// A stream that replays the transactions for the requested hashes.
#[must_use = "streams do nothing unless polled"]
pub struct ReplayTransactionStream<'a> {
    stream: Pin<Box<dyn Stream<Item = ReplayTransactionResult> + 'a>>,
}
impl<'a> Stream for ReplayTransactionStream<'a> {
    type Item = ReplayTransactionResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for ReplayTransactionStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayTransactionStream").finish()
    }
}

#[async_trait::async_trait]
impl<T: TraceApiClient + Sync> TraceApiExt for T {
    type Provider = T;

    fn trace_block_buffered<I, B>(&self, params: I, n: usize) -> TraceBlockStream<'_>
    where
        I: IntoIterator<Item = B>,
        B: Into<BlockId>,
    {
        let blocks = params.into_iter().map(|b| b.into()).collect::<Vec<_>>();
        let stream = futures::stream::iter(blocks.into_iter().map(move |block| async move {
            match self.trace_block(block).await {
                Ok(result) => Ok((result.unwrap_or_default(), block)),
                Err(err) => Err((err, block)),
            }
        }))
        .buffered(n);
        TraceBlockStream { stream: Box::pin(stream) }
    }

    fn trace_block_buffered_unordered<I, B>(&self, params: I, n: usize) -> TraceBlockStream<'_>
    where
        I: IntoIterator<Item = B>,
        B: Into<BlockId>,
    {
        let blocks = params.into_iter().map(|b| b.into()).collect::<Vec<_>>();
        let stream = futures::stream::iter(blocks.into_iter().map(move |block| async move {
            match self.trace_block(block).await {
                Ok(result) => Ok((result.unwrap_or_default(), block)),
                Err(err) => Err((err, block)),
            }
        }))
        .buffer_unordered(n);
        TraceBlockStream { stream: Box::pin(stream) }
    }

    fn replay_transactions<I>(
        &self,
        tx_hashes: I,
        trace_types: HashSet<TraceType>,
    ) -> ReplayTransactionStream<'_>
    where
        I: IntoIterator<Item = TxHash>,
    {
        let hashes = tx_hashes.into_iter().collect::<Vec<_>>();
        let stream = futures::stream::iter(hashes.into_iter().map(move |hash| {
            let trace_types_clone = trace_types.clone(); // Clone outside of the async block
            async move {
                match self.replay_transaction(hash, trace_types_clone).await {
                    Ok(result) => Ok((result, hash)),
                    Err(err) => Err((err, hash)),
                }
            }
        }))
        .buffered(10);
        ReplayTransactionStream { stream: Box::pin(stream) }
    }
}

/// A stream that yields the traces for the requested blocks.
#[must_use = "streams do nothing unless polled"]
pub struct TraceBlockStream<'a> {
    stream: Pin<Box<dyn Stream<Item = TraceBlockResult> + 'a>>,
}

impl<'a> TraceBlockStream<'a> {
    /// Returns the next error result of the stream.
    pub async fn next_err(&mut self) -> Option<(RpcError, BlockId)> {
        loop {
            match self.next().await? {
                Ok(_) => continue,
                Err(err) => return Some(err),
            }
        }
    }
}

impl<'a> Stream for TraceBlockStream<'a> {
    type Item = TraceBlockResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for TraceBlockStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceBlockStream").finish_non_exhaustive()
    }
}

/// A utility to compare RPC responses from two different clients.
///
/// The `RpcComparer` is designed to perform comparisons between two RPC clients.
/// It is useful in scenarios where there's a need to ensure that two different RPC clients
/// return consistent responses. This can be particularly valuable in testing environments
/// where one might want to compare a test client's responses against a production client
/// or compare two different Ethereum client implementations.
#[derive(Debug)]
pub struct RpcComparer<C1, C2>
where
    C1: TraceApiExt,
    C2: TraceApiExt,
{
    client1: C1,
    client2: C2,
}
impl<C1, C2> RpcComparer<C1, C2>
where
    C1: TraceApiExt,
    C2: TraceApiExt,
{
    /// Constructs a new `RpcComparer`.
    ///
    /// Initializes the comparer with two clients that will be used for fetching
    /// and comparison.
    ///
    /// # Arguments
    ///
    /// * `client1` - The first RPC client.
    /// * `client2` - The second RPC client.
    pub fn new(client1: C1, client2: C2) -> Self {
        RpcComparer { client1, client2 }
    }

    /// Compares the `trace_block` responses from the two RPC clients.
    ///
    /// Fetches the `trace_block` responses for the provided block IDs from both clients
    /// and compares them. If there are inconsistencies between the two responses, this
    /// method will panic with a relevant message indicating the difference.
    pub async fn compare_trace_block_responses(&self, block_ids: Vec<BlockId>) {
        let stream1 = self.client1.trace_block_buffered(block_ids.clone(), 2);
        let stream2 = self.client2.trace_block_buffered(block_ids, 2);

        let mut zipped_streams = stream1.zip(stream2);

        while let Some((result1, result2)) = zipped_streams.next().await {
            match (result1, result2) {
                (Ok((ref traces1_data, ref block1)), Ok((ref traces2_data, ref block2))) => {
                    assert_eq!(
                        traces1_data, traces2_data,
                        "Mismatch in traces for block: {:?}",
                        block1
                    );
                    assert_eq!(block1, block2, "Mismatch in block ids.");
                }
                (Err((ref err1, ref block1)), Err((ref err2, ref block2))) => {
                    assert_eq!(
                        format!("{:?}", err1),
                        format!("{:?}", err2),
                        "Different errors for block: {:?}",
                        block1
                    );
                    assert_eq!(block1, block2, "Mismatch in block ids.");
                }
                _ => panic!("One client returned Ok while the other returned Err."),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::http_client::HttpClientBuilder;
    use reth_primitives::BlockNumberOrTag;

    fn assert_is_stream<St: Stream>(_: &St) {}

    #[tokio::test]
    async fn can_create_block_stream() {
        let client = HttpClientBuilder::default().build("http://localhost:8545").unwrap();
        let block = vec![BlockId::Number(5u64.into()), BlockNumberOrTag::Latest.into()];
        let stream = client.trace_block_buffered(block, 2);
        assert_is_stream(&stream);
    }
}
