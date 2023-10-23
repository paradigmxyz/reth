//! Helpers for testing trace calls.
use futures::{Stream, StreamExt};
use jsonrpsee::{core::Error as RpcError, http_client::HttpClientBuilder};
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

//This part is for testing.
//
//------------------------
/// A utility to compare RPC responses from two endpoints.
#[derive(Debug)]
pub struct RpcComparer<'a> {
    endpoint1: &'a str,
    endpoint2: &'a str,
}
impl<'a> RpcComparer<'a> {
    /// Create a new `RpcComparer` instance.
    ///
    /// # Arguments
    ///
    /// * `endpoint1` - URL of the first RPC endpoint.
    /// * `endpoint2` - URL of the second RPC endpoint.
    pub fn new(endpoint1: &'a str, endpoint2: &'a str) -> Self {
        RpcComparer { endpoint1, endpoint2 }
    }
    /// Compare the `trace_block` responses of the two RPC endpoints(basically it should be Reth and
    /// another Client (for our purpose.)).
    ///
    /// This method fetches the `trace_block` responses for the given `block_ids` from both
    /// endpoints and compares them. If any inconsistencies are found, it asserts with the
    /// relevant message.
    ///
    /// # Arguments
    ///
    /// * `block_ids` - A vector of block IDs to compare.
    pub async fn compare_trace_block_responses(&self, block_ids: Vec<BlockId>) {
        let client1 = HttpClientBuilder::default().build(self.endpoint1).unwrap();
        let client2 = HttpClientBuilder::default().build(self.endpoint2).unwrap();

        let stream1 = client1.trace_block_buffered(block_ids.clone(), 2);
        let stream2 = client2.trace_block_buffered(block_ids, 2);

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
                _ => panic!("One endpoint returned Ok while the other returned Err."),
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

    #[tokio::test]
    async fn compare_trace_block_responses() {
        let comparer = RpcComparer::new("http://localhost:8545", "https://eth.llamarpc.com");
        let block_ids = vec![BlockId::Number(5u64.into()), BlockNumberOrTag::Latest.into()];

        comparer.compare_trace_block_responses(block_ids).await;
    }
}
