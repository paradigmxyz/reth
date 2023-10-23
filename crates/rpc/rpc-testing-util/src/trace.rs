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

    async fn compare_endpoints(endpoint1: &str, endpoint2: &str, block_ids: Vec<BlockId>) {
        let client1 = HttpClientBuilder::default().build(endpoint1).unwrap();
        let client2 = HttpClientBuilder::default().build(endpoint2).unwrap();

        let stream1 = client1.trace_block_buffered(block_ids.clone(), 2);
        let stream2 = client2.trace_block_buffered(block_ids, 2);

        let traces1: Vec<_> = stream1.collect().await;
        let traces2: Vec<_> = stream2.collect().await;

        for (trace1, trace2) in traces1.iter().zip(traces2.iter()) {
            match (trace1, trace2) {
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
                _ => panic!(
                    "One endpoint returned Ok while the other returned Err for block: {:?}",
                    trace1
                ),
            }
        }
    }

    //Let's compare between Reth and another client.
    #[tokio::test]
    async fn compare_trace_block_responses() {
        let endpoint1 = "http://localhost:8545"; // I suppose this is Reth0
        let endpoint2 = "https://eth.llamarpc.com"; // Any another client.
        let block_ids = vec![BlockId::Number(5u64.into()), BlockNumberOrTag::Latest.into()];

        compare_endpoints(endpoint1, endpoint2, block_ids).await;
    }
}
