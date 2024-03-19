//! Helpers for testing debug trace calls.

use futures::{Stream, StreamExt};
use jsonrpsee::core::Error as RpcError;
use reth_primitives::{BlockId, TxHash, B256};
use reth_rpc_api::{clients::DebugApiClient, EthApiClient};
use reth_rpc_types::{
    trace::{
        common::TraceResult,
        geth::{GethDebugTracerType, GethDebugTracingOptions, GethTrace},
    },
    TransactionRequest,
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

const NOOP_TRACER: &str = include_str!("../assets/noop-tracer.js");
const JS_TRACER_TEMPLATE: &str = include_str!("../assets/tracer-template.js");

/// A result type for the `debug_trace_transaction` method that also captures the requested hash.
pub type TraceTransactionResult = Result<(serde_json::Value, TxHash), (RpcError, TxHash)>;

/// A result type for the `debug_trace_block` method that also captures the requested block.
pub type DebugTraceBlockResult =
    Result<(Vec<TraceResult<GethTrace, String>>, BlockId), (RpcError, BlockId)>;

/// An extension trait for the Trace API.
pub trait DebugApiExt {
    /// The provider type that is used to make the requests.
    type Provider;

    /// Same as [DebugApiClient::debug_trace_transaction] but returns the result as json.
    fn debug_trace_transaction_json(
        &self,
        hash: B256,
        opts: GethDebugTracingOptions,
    ) -> impl Future<Output = Result<serde_json::Value, jsonrpsee::core::Error>> + Send;

    /// Trace all transactions in a block individually with the given tracing opts.
    fn debug_trace_transactions_in_block<B>(
        &self,
        block: B,
        opts: GethDebugTracingOptions,
    ) -> impl Future<Output = Result<DebugTraceTransactionsStream<'_>, jsonrpsee::core::Error>> + Send
    where
        B: Into<BlockId> + Send;

    /// Trace all given blocks with the given tracing opts, returning a stream.
    fn debug_trace_block_buffered_unordered<I, B>(
        &self,
        params: I,
        opts: Option<GethDebugTracingOptions>,
        n: usize,
    ) -> DebugTraceBlockStream<'_>
    where
        I: IntoIterator<Item = B>,
        B: Into<BlockId> + Send;

    ///  method  for debug_traceCall
    fn debug_trace_call_json(
        &self,
        request: TransactionRequest,
        opts: GethDebugTracingOptions,
    ) -> impl Future<Output = Result<serde_json::Value, jsonrpsee::core::Error>> + Send;

    ///  method for debug_traceCall using raw JSON strings for the request and options.
    fn debug_trace_call_raw_json(
        &self,
        request_json: String,
        opts_json: String,
    ) -> impl Future<Output = Result<serde_json::Value, RpcError>> + Send;
}

impl<T: DebugApiClient + Sync> DebugApiExt for T
where
    T: EthApiClient,
{
    type Provider = T;

    async fn debug_trace_transaction_json(
        &self,
        hash: B256,
        opts: GethDebugTracingOptions,
    ) -> Result<serde_json::Value, jsonrpsee::core::Error> {
        let mut params = jsonrpsee::core::params::ArrayParams::new();
        params.insert(hash).unwrap();
        params.insert(opts).unwrap();
        self.request("debug_traceTransaction", params).await
    }

    async fn debug_trace_transactions_in_block<B>(
        &self,
        block: B,
        opts: GethDebugTracingOptions,
    ) -> Result<DebugTraceTransactionsStream<'_>, jsonrpsee::core::Error>
    where
        B: Into<BlockId> + Send,
    {
        let block = match block.into() {
            BlockId::Hash(hash) => self.block_by_hash(hash.block_hash, false).await,
            BlockId::Number(tag) => self.block_by_number(tag, false).await,
        }?
        .ok_or_else(|| RpcError::Custom("block not found".to_string()))?;
        let hashes = block.transactions.hashes().map(|tx| (*tx, opts.clone())).collect::<Vec<_>>();
        let stream = futures::stream::iter(hashes.into_iter().map(move |(tx, opts)| async move {
            match self.debug_trace_transaction_json(tx, opts).await {
                Ok(result) => Ok((result, tx)),
                Err(err) => Err((err, tx)),
            }
        }))
        .buffered(10);

        Ok(DebugTraceTransactionsStream { stream: Box::pin(stream) })
    }

    fn debug_trace_block_buffered_unordered<I, B>(
        &self,
        params: I,
        opts: Option<GethDebugTracingOptions>,
        n: usize,
    ) -> DebugTraceBlockStream<'_>
    where
        I: IntoIterator<Item = B>,
        B: Into<BlockId> + Send,
    {
        let blocks =
            params.into_iter().map(|block| (block.into(), opts.clone())).collect::<Vec<_>>();
        let stream =
            futures::stream::iter(blocks.into_iter().map(move |(block, opts)| async move {
                let trace_future = match block {
                    BlockId::Hash(hash) => {
                        self.debug_trace_block_by_hash(hash.block_hash, opts.clone())
                    }
                    BlockId::Number(tag) => self.debug_trace_block_by_number(tag, opts.clone()),
                };

                match trace_future.await {
                    Ok(result) => Ok((result, block)),
                    Err(err) => Err((err, block)),
                }
            }))
            .buffer_unordered(n);
        DebugTraceBlockStream { stream: Box::pin(stream) }
    }

    async fn debug_trace_call_json(
        &self,
        request: TransactionRequest,
        opts: GethDebugTracingOptions,
    ) -> Result<serde_json::Value, jsonrpsee::core::Error> {
        let mut params = jsonrpsee::core::params::ArrayParams::new();
        params.insert(request).unwrap();
        params.insert(opts).unwrap();
        self.request("debug_traceCall", params).await
    }

    async fn debug_trace_call_raw_json(
        &self,
        request_json: String,
        opts_json: String,
    ) -> Result<serde_json::Value, RpcError> {
        let request = serde_json::from_str::<TransactionRequest>(&request_json)
            .map_err(|e| RpcError::Custom(e.to_string()))?;
        let opts = serde_json::from_str::<GethDebugTracingOptions>(&opts_json)
            .map_err(|e| RpcError::Custom(e.to_string()))?;

        self.debug_trace_call_json(request, opts).await
    }
}

/// A helper type that can be used to build a javascript tracer.
#[derive(Debug, Clone, Default)]
pub struct JsTracerBuilder {
    /// `setup_body` is invoked once at the beginning, during the construction of a given
    /// transaction.
    setup_body: Option<String>,

    /// `fault_body` is invoked when an error happens during the execution of an opcode which
    /// wasn't reported in step.
    fault_body: Option<String>,

    /// `result_body` returns a JSON-serializable value to the RPC caller.
    result_body: Option<String>,

    /// `enter_body` is invoked on stepping in of an internal call.
    enter_body: Option<String>,

    /// `step_body` is called for each step of the EVM, or when an error occurs, as the specified
    /// transaction is traced.
    step_body: Option<String>,

    /// `exit_body` is invoked on stepping out of an internal call.
    exit_body: Option<String>,
}

impl JsTracerBuilder {
    /// Sets the body of the fault function
    ///
    /// The body code has access to the `log` and `db` variables.
    pub fn fault_body(mut self, body: impl Into<String>) -> Self {
        self.fault_body = Some(body.into());
        self
    }

    /// Sets the body of the setup function
    ///
    /// This body includes the `cfg` object variable
    pub fn setup_body(mut self, body: impl Into<String>) -> Self {
        self.setup_body = Some(body.into());
        self
    }

    /// Sets the body of the result function
    ///
    /// The body code has access to the `ctx` and `db` variables.
    ///
    /// ```
    /// use reth_rpc_api_testing_util::debug::JsTracerBuilder;
    /// let code = JsTracerBuilder::default().result_body("return {};").code();
    /// ```
    pub fn result_body(mut self, body: impl Into<String>) -> Self {
        self.result_body = Some(body.into());
        self
    }

    /// Sets the body of the enter function
    ///
    /// The body code has access to the `frame` variable.
    pub fn enter_body(mut self, body: impl Into<String>) -> Self {
        self.enter_body = Some(body.into());
        self
    }

    /// Sets the body of the step function
    ///
    /// The body code has access to the `log` and `db` variables.
    pub fn step_body(mut self, body: impl Into<String>) -> Self {
        self.step_body = Some(body.into());
        self
    }

    /// Sets the body of the exit function
    ///
    /// The body code has access to the `res` variable.
    pub fn exit_body(mut self, body: impl Into<String>) -> Self {
        self.exit_body = Some(body.into());
        self
    }

    /// Returns the tracers JS code
    pub fn code(self) -> String {
        let mut template = JS_TRACER_TEMPLATE.to_string();
        template = template.replace("//<setup>", self.setup_body.as_deref().unwrap_or_default());
        template = template.replace("//<fault>", self.fault_body.as_deref().unwrap_or_default());
        template =
            template.replace("//<result>", self.result_body.as_deref().unwrap_or("return {};"));
        template = template.replace("//<step>", self.step_body.as_deref().unwrap_or_default());
        template = template.replace("//<enter>", self.enter_body.as_deref().unwrap_or_default());
        template = template.replace("//<exit>", self.exit_body.as_deref().unwrap_or_default());
        template
    }
}

impl std::fmt::Display for JsTracerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.clone().code())
    }
}

impl From<JsTracerBuilder> for GethDebugTracingOptions {
    fn from(b: JsTracerBuilder) -> Self {
        GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::JsTracer(b.code())),
            tracer_config: serde_json::Value::Object(Default::default()).into(),
            ..Default::default()
        }
    }
}
impl From<JsTracerBuilder> for Option<GethDebugTracingOptions> {
    fn from(b: JsTracerBuilder) -> Self {
        Some(b.into())
    }
}

/// A stream that yields the traces for the requested blocks.
#[must_use = "streams do nothing unless polled"]
pub struct DebugTraceTransactionsStream<'a> {
    stream: Pin<Box<dyn Stream<Item = TraceTransactionResult> + 'a>>,
}

impl<'a> DebugTraceTransactionsStream<'a> {
    /// Returns the next error result of the stream.
    pub async fn next_err(&mut self) -> Option<(RpcError, TxHash)> {
        loop {
            match self.next().await? {
                Ok(_) => continue,
                Err(err) => return Some(err),
            }
        }
    }
}

impl<'a> Stream for DebugTraceTransactionsStream<'a> {
    type Item = TraceTransactionResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for DebugTraceTransactionsStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugTraceTransactionsStream").finish_non_exhaustive()
    }
}

/// A stream that yields the `debug_` traces for the requested blocks.
#[must_use = "streams do nothing unless polled"]
pub struct DebugTraceBlockStream<'a> {
    stream: Pin<Box<dyn Stream<Item = DebugTraceBlockResult> + 'a>>,
}

impl<'a> DebugTraceBlockStream<'a> {
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

impl<'a> Stream for DebugTraceBlockStream<'a> {
    type Item = DebugTraceBlockResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<'a> std::fmt::Debug for DebugTraceBlockStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugTraceBlockStream").finish_non_exhaustive()
    }
}

/// A javascript tracer that does nothing
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct NoopJsTracer;

impl From<NoopJsTracer> for GethDebugTracingOptions {
    fn from(_: NoopJsTracer) -> Self {
        GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::JsTracer(NOOP_TRACER.to_string())),
            tracer_config: serde_json::Value::Object(Default::default()).into(),
            ..Default::default()
        }
    }
}
impl From<NoopJsTracer> for Option<GethDebugTracingOptions> {
    fn from(_: NoopJsTracer) -> Self {
        Some(NoopJsTracer.into())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        debug::{DebugApiExt, JsTracerBuilder, NoopJsTracer},
        utils::parse_env_url,
    };
    use futures::StreamExt;
    use jsonrpsee::http_client::HttpClientBuilder;
    use reth_rpc_types::trace::geth::{CallConfig, GethDebugTracingOptions};

    // random tx <https://sepolia.etherscan.io/tx/0x5525c63a805df2b83c113ebcc8c7672a3b290673c4e81335b410cd9ebc64e085>
    const TX_1: &str = "0x5525c63a805df2b83c113ebcc8c7672a3b290673c4e81335b410cd9ebc64e085";

    #[tokio::test]
    #[ignore]
    async fn can_trace_noop_sepolia() {
        let tx = TX_1.parse().unwrap();
        let url = parse_env_url("RETH_RPC_TEST_NODE_URL").unwrap();
        let client = HttpClientBuilder::default().build(url).unwrap();
        let res =
            client.debug_trace_transaction_json(tx, NoopJsTracer::default().into()).await.unwrap();
        assert_eq!(res, serde_json::Value::Object(Default::default()));
    }

    #[tokio::test]
    #[ignore]
    async fn can_trace_default_template() {
        let tx = TX_1.parse().unwrap();
        let url = parse_env_url("RETH_RPC_TEST_NODE_URL").unwrap();
        let client = HttpClientBuilder::default().build(url).unwrap();
        let res = client
            .debug_trace_transaction_json(tx, JsTracerBuilder::default().into())
            .await
            .unwrap();
        assert_eq!(res, serde_json::Value::Object(Default::default()));
    }

    #[tokio::test]
    #[ignore]
    async fn can_debug_trace_block_transactions() {
        let block = 11_117_104u64;
        let url = parse_env_url("RETH_RPC_TEST_NODE_URL").unwrap();
        let client = HttpClientBuilder::default().build(url).unwrap();

        let opts =
            GethDebugTracingOptions::default().call_config(CallConfig::default().only_top_call());

        let mut stream = client.debug_trace_transactions_in_block(block, opts).await.unwrap();
        while let Some(res) = stream.next().await {
            if let Err((err, tx)) = res {
                println!("failed to trace {tx:?}  {err}");
            }
        }
    }
}
