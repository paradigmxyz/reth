//! Helpers for testing trace calls.
use futures::{Stream, StreamExt};
use jsonrpsee::core::Error as RpcError;
use reth_primitives::{BlockId, Bytes, TxHash, B256};
use reth_rpc_api::clients::TraceApiClient;
use reth_rpc_types::{
    trace::{
        filter::TraceFilter,
        parity::{LocalizedTransactionTrace, TraceResults, TraceType},
        tracerequest::TraceCallRequest,
    },
    CallRequest, Index,
};
use std::{
    collections::HashSet,
    pin::Pin,
    task::{Context, Poll},
};
/// A type alias that represents the result of a raw transaction trace stream.
type RawTransactionTraceResult<'a> =
    Pin<Box<dyn Stream<Item = Result<(TraceResults, Bytes), (RpcError, Bytes)>> + 'a>>;
/// A result type for the `trace_block` method that also captures the requested block.
pub type TraceBlockResult = Result<(Vec<LocalizedTransactionTrace>, BlockId), (RpcError, BlockId)>;
/// Type alias representing the result of replaying a transaction.

pub type ReplayTransactionResult = Result<(TraceResults, TxHash), (RpcError, TxHash)>;

/// A type representing the result of calling `trace_call_many` method.

pub type CallManyTraceResult = Result<
    (Vec<TraceResults>, Vec<(CallRequest, HashSet<TraceType>)>),
    (RpcError, Vec<(CallRequest, HashSet<TraceType>)>),
>;
/// Result type for the `trace_get` method that also captures the requested transaction hash and
/// index.
pub type TraceGetResult =
    Result<(Option<LocalizedTransactionTrace>, B256, Vec<Index>), (RpcError, B256, Vec<Index>)>;
/// Represents a result type for the `trace_filter` stream extension.
pub type TraceFilterResult =
    Result<(Vec<LocalizedTransactionTrace>, TraceFilter), (RpcError, TraceFilter)>;
/// Represents the result of a single trace call.
pub type TraceCallResult = Result<TraceResults, (RpcError, TraceCallRequest)>;

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

    /// Returns a new stream that traces the provided raw transaction data.
    fn trace_raw_transaction_stream(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> RawTransactionTraceStream<'_>;
    /// Creates a stream of results for multiple dependent transaction calls on top of the same
    /// block.

    fn trace_call_many_stream<I>(
        &self,
        calls: I,
        block_id: Option<BlockId>,
    ) -> CallManyTraceStream<'_>
    where
        I: IntoIterator<Item = (CallRequest, HashSet<TraceType>)>;
    /// Returns a new stream that yields the traces for the given transaction hash and indices.
    fn trace_get_stream<I>(&self, hash: B256, indices: I) -> TraceGetStream<'_>
    where
        I: IntoIterator<Item = Index>;

    /// Returns a new stream that yields traces for given filters.
    fn trace_filter_stream<I>(&self, filters: I) -> TraceFilterStream<'_>
    where
        I: IntoIterator<Item = TraceFilter>;
    /// Returns a new stream that yields the trace results for the given call requests.
    fn trace_call_stream(&self, request: TraceCallRequest) -> TraceCallStream<'_>;
}
/// `TraceCallStream` provides an asynchronous stream of tracing results.

#[must_use = "streams do nothing unless polled"]
pub struct TraceCallStream<'a> {
    stream: Pin<Box<dyn Stream<Item = TraceCallResult> + 'a>>,
}
impl<'a> Stream for TraceCallStream<'a> {
    type Item = TraceCallResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for TraceCallStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceCallStream").finish()
    }
}

/// Represents a stream that asynchronously yields the results of the `trace_filter` method.
#[must_use = "streams do nothing unless polled"]
pub struct TraceFilterStream<'a> {
    stream: Pin<Box<dyn Stream<Item = TraceFilterResult> + 'a>>,
}

impl<'a> Stream for TraceFilterStream<'a> {
    type Item = TraceFilterResult;

    /// Attempts to pull out the next value of the stream.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for TraceFilterStream<'a> {
    /// Provides a debug representation of the `TraceFilterStream`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceFilterStream").finish_non_exhaustive()
    }
}
/// A stream that asynchronously yields the results of the `trace_get` method for a given
/// transaction hash and a series of indices.
#[must_use = "streams do nothing unless polled"]
pub struct TraceGetStream<'a> {
    stream: Pin<Box<dyn Stream<Item = TraceGetResult> + 'a>>,
}
impl<'a> Stream for TraceGetStream<'a> {
    type Item = TraceGetResult;
    /// Attempts to pull out the next item of the stream
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for TraceGetStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceGetStream").finish_non_exhaustive()
    }
}

/// A stream that provides asynchronous iteration over results from the `trace_call_many` function.
///
/// The stream yields items of type `CallManyTraceResult`.
#[must_use = "streams do nothing unless polled"]
pub struct CallManyTraceStream<'a> {
    stream: Pin<Box<dyn Stream<Item = CallManyTraceResult> + 'a>>,
}

impl<'a> Stream for CallManyTraceStream<'a> {
    type Item = CallManyTraceResult;
    /// Polls for the next item from the stream.

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for CallManyTraceStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallManyTraceStream").finish()
    }
}

/// A stream that traces the provided raw transaction data.

#[must_use = "streams do nothing unless polled"]
pub struct RawTransactionTraceStream<'a> {
    stream: RawTransactionTraceResult<'a>,
}

impl<'a> Stream for RawTransactionTraceStream<'a> {
    type Item = Result<(TraceResults, Bytes), (RpcError, Bytes)>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for RawTransactionTraceStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawTransactionTraceStream").finish()
    }
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
    fn trace_raw_transaction_stream(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> RawTransactionTraceStream<'_> {
        let stream = futures::stream::once(async move {
            match self.trace_raw_transaction(data.clone(), trace_types, block_id).await {
                Ok(result) => Ok((result, data)),
                Err(err) => Err((err, data)),
            }
        });
        RawTransactionTraceStream { stream: Box::pin(stream) }
    }

    fn trace_call_many_stream<I>(
        &self,
        calls: I,
        block_id: Option<BlockId>,
    ) -> CallManyTraceStream<'_>
    where
        I: IntoIterator<Item = (CallRequest, HashSet<TraceType>)>,
    {
        let call_set = calls.into_iter().collect::<Vec<_>>();
        let stream = futures::stream::once(async move {
            match self.trace_call_many(call_set.clone(), block_id).await {
                Ok(results) => Ok((results, call_set)),
                Err(err) => Err((err, call_set)),
            }
        });
        CallManyTraceStream { stream: Box::pin(stream) }
    }

    fn trace_get_stream<I>(&self, hash: B256, indices: I) -> TraceGetStream<'_>
    where
        I: IntoIterator<Item = Index>,
    {
        let index_list = indices.into_iter().collect::<Vec<_>>();
        let stream = futures::stream::iter(index_list.into_iter().map(move |index| async move {
            match self.trace_get(hash, vec![index]).await {
                Ok(result) => Ok((result, hash, vec![index])),
                Err(err) => Err((err, hash, vec![index])),
            }
        }))
        .buffered(10);
        TraceGetStream { stream: Box::pin(stream) }
    }

    fn trace_filter_stream<I>(&self, filters: I) -> TraceFilterStream<'_>
    where
        I: IntoIterator<Item = TraceFilter>,
    {
        let filter_list = filters.into_iter().collect::<Vec<_>>();
        let stream = futures::stream::iter(filter_list.into_iter().map(move |filter| async move {
            match self.trace_filter(filter.clone()).await {
                Ok(result) => Ok((result, filter)),
                Err(err) => Err((err, filter)),
            }
        }))
        .buffered(10);
        TraceFilterStream { stream: Box::pin(stream) }
    }

    fn trace_call_stream(&self, request: TraceCallRequest) -> TraceCallStream<'_> {
        let stream = futures::stream::once(async move {
            match self
                .trace_call(
                    request.call.clone(),
                    request.trace_types.clone(),
                    request.block_id,
                    request.state_overrides.clone(),
                    request.block_overrides.clone(),
                )
                .await
            {
                Ok(result) => Ok(result),
                Err(err) => Err((err, request)),
            }
        });
        TraceCallStream { stream: Box::pin(stream) }
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
                    similar_asserts::assert_eq!(
                        traces1_data,
                        traces2_data,
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

    /// Compares the `replay_transactions` responses from the two RPC clients.
    pub async fn compare_replay_transaction_responses(
        &self,
        transaction_hashes: Vec<TxHash>,
        trace_types: HashSet<TraceType>,
    ) {
        let stream1 =
            self.client1.replay_transactions(transaction_hashes.clone(), trace_types.clone());
        let stream2 = self.client2.replay_transactions(transaction_hashes, trace_types);

        let mut zipped_streams = stream1.zip(stream2);

        while let Some((result1, result2)) = zipped_streams.next().await {
            match (result1, result2) {
                (Ok((ref trace1_data, ref tx_hash1)), Ok((ref trace2_data, ref tx_hash2))) => {
                    similar_asserts::assert_eq!(
                        trace1_data,
                        trace2_data,
                        "Mismatch in trace results for transaction: {:?}",
                        tx_hash1
                    );
                    assert_eq!(tx_hash1, tx_hash2, "Mismatch in transaction hashes.");
                }
                (Err((ref err1, ref tx_hash1)), Err((ref err2, ref tx_hash2))) => {
                    assert_eq!(
                        format!("{:?}", err1),
                        format!("{:?}", err2),
                        "Different errors for transaction: {:?}",
                        tx_hash1
                    );
                    assert_eq!(tx_hash1, tx_hash2, "Mismatch in transaction hashes.");
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
    use reth_rpc_types::trace::filter::TraceFilterMode;
    use std::collections::HashSet;

    fn assert_is_stream<St: Stream>(_: &St) {}

    #[tokio::test]
    async fn can_create_block_stream() {
        let client = HttpClientBuilder::default().build("http://localhost:8545").unwrap();
        let block = vec![BlockId::Number(5u64.into()), BlockNumberOrTag::Latest.into()];
        let stream = client.trace_block_buffered(block, 2);
        assert_is_stream(&stream);
    }

    #[tokio::test]
    #[ignore]
    async fn can_create_replay_transaction_stream() {
        let client = HttpClientBuilder::default().build("http://localhost:8545").unwrap();

        // Assuming you have some transactions you want to test, replace with actual hashes.
        let transactions = vec![
            "0x4e08fe36db723a338e852f89f613e606b0c9a17e649b18b01251f86236a2cef3".parse().unwrap(),
            "0xea2817f1aeeb587b82f4ab87a6dbd3560fc35ed28de1be280cb40b2a24ab48bb".parse().unwrap(),
        ];

        let trace_types = HashSet::from([TraceType::StateDiff, TraceType::VmTrace]);

        let mut stream = client.replay_transactions(transactions, trace_types);
        let mut successes = 0;
        let mut failures = 0;
        let mut all_results = Vec::new();

        assert_is_stream(&stream);

        while let Some(result) = stream.next().await {
            match result {
                Ok((trace_result, tx_hash)) => {
                    println!("Success for tx_hash {:?}: {:?}", tx_hash, trace_result);
                    successes += 1;
                    all_results.push(Ok((trace_result, tx_hash)));
                }
                Err((error, tx_hash)) => {
                    println!("Error for tx_hash {:?}: {:?}", tx_hash, error);
                    failures += 1;
                    all_results.push(Err((error, tx_hash)));
                }
            }
        }

        println!("Total successes: {}", successes);
        println!("Total failures: {}", failures);
    }

    #[tokio::test]
    #[ignore]
    async fn can_create_trace_call_many_stream() {
        let client = HttpClientBuilder::default().build("http://localhost:8545").unwrap();

        let call_request_1 = CallRequest::default();
        let call_request_2 = CallRequest::default();
        let trace_types = HashSet::from([TraceType::StateDiff, TraceType::VmTrace]);
        let calls = vec![(call_request_1, trace_types.clone()), (call_request_2, trace_types)];

        let mut stream = client.trace_call_many_stream(calls, None);

        assert_is_stream(&stream);

        while let Some(result) = stream.next().await {
            match result {
                Ok(trace_result) => {
                    println!("Success: {:?}", trace_result);
                }
                Err(error) => {
                    println!("Error: {:?}", error);
                }
            }
        }
    }
    #[tokio::test]
    #[ignore]
    async fn can_create_trace_get_stream() {
        let client = HttpClientBuilder::default().build("http://localhost:8545").unwrap();

        let tx_hash: B256 = "".parse().unwrap();

        let indices: Vec<Index> = vec![Index::from(0)];

        let mut stream = client.trace_get_stream(tx_hash, indices);

        while let Some(result) = stream.next().await {
            match result {
                Ok(trace) => {
                    println!("Received trace: {:?}", trace);
                }
                Err(e) => {
                    println!("Error fetching trace: {:?}", e);
                }
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn can_create_trace_filter() {
        let client = HttpClientBuilder::default().build("http://localhost:8545").unwrap();

        let filter = TraceFilter {
            from_block: None,
            to_block: None,
            from_address: Vec::new(),
            to_address: Vec::new(),
            mode: TraceFilterMode::Union,
            after: None,
            count: None,
        };

        let filters = vec![filter];
        let mut stream = client.trace_filter_stream(filters);

        while let Some(result) = stream.next().await {
            match result {
                Ok(trace) => {
                    println!("Received trace: {:?}", trace);
                }
                Err(e) => {
                    println!("Error fetching trace: {:?}", e);
                }
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn can_create_trace_call_stream() {
        let client = HttpClientBuilder::default().build("http://localhost:8545").unwrap();

        let trace_call_request = TraceCallRequest::default();

        let mut stream = client.trace_call_stream(trace_call_request);
        let mut successes = 0;
        let mut failures = 0;
        let mut all_results = Vec::new();

        assert_is_stream(&stream);

        while let Some(result) = stream.next().await {
            match result {
                Ok(trace_result) => {
                    println!("Success: {:?}", trace_result);
                    successes += 1;
                    all_results.push(Ok(trace_result));
                }
                Err((error, request)) => {
                    println!("Error for request {:?}: {:?}", request, error);
                    failures += 1;
                    all_results.push(Err((error, request)));
                }
            }
        }

        println!("Total successes: {}", successes);
        println!("Total failures: {}", failures);
    }
}
