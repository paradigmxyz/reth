use jsonrpsee::{
    core::middleware::{Batch, Notification},
    server::middleware::rpc::RpcServiceT,
    types::Request,
    MethodResponse, RpcModule,
};

// use jsonrpsee::core::{
//     types::{Id,
// use crate::{params::{Id}};
// at the top of metrics.rs (or a dedicated utils.rs)
use jsonrpsee::core::__reexports::serde_json::{self, Value as JsonValue};

use alloy_primitives::B256;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::RpcResult;
use jsonrpsee::http_client::HttpClient;
use jsonrpsee::types::{
    error::{ErrorCode, ErrorObjectOwned},
    Params,
};
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use reth_primitives_traits::AlloyBlockHeader;
use serde_json::value::RawValue;
use std::str::FromStr;

use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::BlockNumber;
use reth_storage_api::BlockReaderIdExt;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};
use tower::Layer;

/// Metrics for the RPC server.
///
/// Metrics are divided into two categories:
/// - Connection metrics: metrics for the connection (e.g. number of connections opened, relevant
///   for WS and IPC)
/// - Request metrics: metrics for each RPC method (e.g. number of calls started, time taken to
///   process a call)
#[derive(Default, Debug, Clone)]
pub(crate) struct RpcRequestMetrics {
    inner: Arc<RpcServerMetricsInner>,
}

impl RpcRequestMetrics {
    pub(crate) fn new(module: &RpcModule<()>, transport: RpcTransport) -> Self {
        Self {
            inner: Arc::new(RpcServerMetricsInner {
                connection_metrics: transport.connection_metrics(),
                call_metrics: module
                    .method_names()
                    .map(|method| {
                        (method, RpcServerCallMetrics::new_with_labels(&[("method", method)]))
                    })
                    .collect(),
            }),
        }
    }

    /// Creates a new instance of the metrics layer for HTTP.
    pub(crate) fn http(module: &RpcModule<()>) -> Self {
        Self::new(module, RpcTransport::Http)
    }

    /// Creates a new instance of the metrics layer for same port.
    ///
    /// Note: currently it's not possible to track transport specific metrics for a server that runs http and ws on the same port: <https://github.com/paritytech/jsonrpsee/issues/1345> until we have this feature we will use the http metrics for this case.
    pub(crate) fn same_port(module: &RpcModule<()>) -> Self {
        Self::http(module)
    }

    /// Creates a new instance of the metrics layer for Ws.
    pub(crate) fn ws(module: &RpcModule<()>) -> Self {
        Self::new(module, RpcTransport::WebSocket)
    }

    /// Creates a new instance of the metrics layer for Ws.
    pub(crate) fn ipc(module: &RpcModule<()>) -> Self {
        Self::new(module, RpcTransport::Ipc)
    }
}

impl<S> Layer<S> for RpcRequestMetrics {
    type Service = RpcRequestMetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RpcRequestMetricsService::new(inner, self.clone())
    }
}

/// Metrics for the RPC server
#[derive(Default, Clone, Debug)]
struct RpcServerMetricsInner {
    /// Connection metrics per transport type
    connection_metrics: RpcServerConnectionMetrics,
    /// Call metrics per RPC method
    call_metrics: HashMap<&'static str, RpcServerCallMetrics>,
}

/// A [`RpcServiceT`] middleware that captures RPC metrics for the server.
///
/// This is created per connection and captures metrics for each request.
#[derive(Clone, Debug)]
pub struct RpcRequestMetricsService<S> {
    /// The metrics collector for RPC requests
    metrics: RpcRequestMetrics,
    /// The inner service being wrapped
    inner: S,
}

impl<S> RpcRequestMetricsService<S> {
    pub(crate) fn new(service: S, metrics: RpcRequestMetrics) -> Self {
        // this instance is kept alive for the duration of the connection
        metrics.inner.connection_metrics.connections_opened_total.increment(1);
        Self { inner: service, metrics }
    }
}

impl<S> RpcServiceT for RpcRequestMetricsService<S>
where
    S: RpcServiceT<MethodResponse = MethodResponse> + Send + Sync + Clone + 'static,
{
    type MethodResponse = S::MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = S::MethodResponse> + Send + 'a {
        self.metrics.inner.connection_metrics.requests_started_total.increment(1);
        let call_metrics = self.metrics.inner.call_metrics.get_key_value(req.method.as_ref());
        if let Some((_, call_metrics)) = &call_metrics {
            call_metrics.started_total.increment(1);
        }
        MeteredRequestFuture {
            fut: self.inner.call(req),
            started_at: Instant::now(),
            metrics: self.metrics.clone(),
            method: call_metrics.map(|(method, _)| *method),
        }
    }

    fn batch<'a>(&self, req: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        self.inner.batch(req)
    }

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        self.inner.notification(n)
    }
}

impl<S> Drop for RpcRequestMetricsService<S> {
    fn drop(&mut self) {
        // update connection metrics, connection closed
        self.metrics.inner.connection_metrics.connections_closed_total.increment(1);
    }
}

/// Response future to update the metrics for a single request/response pair.
#[pin_project::pin_project]
pub struct MeteredRequestFuture<F> {
    #[pin]
    fut: F,
    /// time when the request started
    started_at: Instant,
    /// metrics for the method call
    metrics: RpcRequestMetrics,
    /// the method name if known
    method: Option<&'static str>,
}

impl<F> std::fmt::Debug for MeteredRequestFuture<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MeteredRequestFuture")
    }
}

impl<F: Future<Output = MethodResponse>> Future for MeteredRequestFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        let res = this.fut.poll(cx);
        if let Poll::Ready(resp) = &res {
            let elapsed = this.started_at.elapsed().as_secs_f64();

            // update transport metrics
            this.metrics.inner.connection_metrics.requests_finished_total.increment(1);
            this.metrics.inner.connection_metrics.request_time_seconds.record(elapsed);

            // update call metrics
            if let Some(call_metrics) =
                this.method.and_then(|method| this.metrics.inner.call_metrics.get(method))
            {
                call_metrics.time_seconds.record(elapsed);
                if resp.is_success() {
                    call_metrics.successful_total.increment(1);
                } else {
                    call_metrics.failed_total.increment(1);
                }
            }
        }
        res
    }
}

/// The transport protocol used for the RPC connection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum RpcTransport {
    Http,
    WebSocket,
    Ipc,
}

impl RpcTransport {
    /// Returns the string representation of the transport protocol.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::WebSocket => "ws",
            Self::Ipc => "ipc",
        }
    }

    /// Returns the connection metrics for the transport protocol.
    fn connection_metrics(&self) -> RpcServerConnectionMetrics {
        RpcServerConnectionMetrics::new_with_labels(&[("transport", self.as_str())])
    }
}

/// Metrics for the RPC connections
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server.connections")]
struct RpcServerConnectionMetrics {
    /// The number of connections opened
    connections_opened_total: Counter,
    /// The number of connections closed
    connections_closed_total: Counter,
    /// The number of requests started
    requests_started_total: Counter,
    /// The number of requests finished
    requests_finished_total: Counter,
    /// Response for a single request/response pair
    request_time_seconds: Histogram,
}

/// Metrics for the RPC calls
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server.calls")]
struct RpcServerCallMetrics {
    /// The number of calls started
    started_total: Counter,
    /// The number of successful calls
    successful_total: Counter,
    /// The number of failed calls
    failed_total: Counter,
    /// Response for a single call
    time_seconds: Histogram,
}

/// A service that intercepts RPC calls and forwards pre-bedrock historical requests
/// to a dedicated endpoint.
#[derive(Debug, Clone)]
pub struct HistoricalRpcService<S, P> {
    /// The inner service that handles regular RPC requests
    inner: S,
    /// Client used to forward historical requests  
    historical_client: Arc<HttpClient>,
    /// Provider used to determine if a block is pre-bedrock
    provider: Arc<P>,
    /// Bedrock transition block number
    bedrock_block: BlockNumber,
}
fn parse_block_id_from_params(
    params: &jsonrpsee::types::Params<'_>,
    position: usize,
) -> RpcResult<BlockId> {
    let values: Vec<serde_json::Value> = params.parse()?;
    let val = match values.get(position) {
        Some(v) => v,                                                 // normal path
        None => return Ok(BlockId::Number(BlockNumberOrTag::Latest)), // default
    };

    if let Some(s) = val.as_str() {
        match s {
            "latest" => return Ok(BlockId::Number(BlockNumberOrTag::Latest)),
            "earliest" => return Ok(BlockId::Number(BlockNumberOrTag::Earliest)),
            "pending" => return Ok(BlockId::Number(BlockNumberOrTag::Pending)),
            "finalized" => return Ok(BlockId::Number(BlockNumberOrTag::Finalized)),
            "safe" => return Ok(BlockId::Number(BlockNumberOrTag::Safe)),
            _ => {}
        }

        if s.starts_with("0x") {
            if s.len() == 66 {
                if let Ok(hash) = B256::from_str(s) {
                    return Ok(BlockId::Hash(hash.into()));
                }
            }

            if let Ok(n) = u64::from_str_radix(&s[2..], 16) {
                return Ok(BlockId::Number(BlockNumberOrTag::Number(n.into())));
            }
        }
    }

    if let Some(n) = val.as_u64() {
        return Ok(BlockId::Number(BlockNumberOrTag::Number(n.into())));
    }

    Err(ErrorObjectOwned::from(ErrorCode::InvalidParams))
}

fn get_block_id_from_tx_hash<P>(
    provider: &Arc<P>,
    params: &jsonrpsee::types::Params<'_>,
) -> Option<BlockId>
where
    P: BlockReaderIdExt + Send + Sync,
{
    let tx_hash = params.sequence().next().ok().flatten()?;

    let maybe_tx_with_meta = provider.transaction_by_hash_with_meta(tx_hash).ok()?;

    maybe_tx_with_meta.map(|(_transaction, meta)| BlockId::from(meta.block_hash))
}
fn params_as_vec(params: &Params<'_>) -> Vec<JsonValue> {
    params.parse::<Vec<JsonValue>>().unwrap_or_default()
}

impl<S, P> RpcServiceT for HistoricalRpcService<S, P>
where
    S: RpcServiceT<MethodResponse = MethodResponse> + Send + Sync + Clone + 'static,

    P: BlockReaderIdExt + Send + Sync + Clone + 'static,
{
    type MethodResponse = S::MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        let method_owned: String = req.method.clone().into_owned();
        let params_owned = req.params().into_owned();
        let id_owned: jsonrpsee::types::Id<'static> = match req.id() {
            jsonrpsee::types::Id::Number(n) => jsonrpsee::types::Id::Number(n),
            jsonrpsee::types::Id::Str(s) => jsonrpsee::types::Id::Str(s.into_owned().into()),
            jsonrpsee::types::Id::Null => jsonrpsee::types::Id::Null,
        };

        let inner_service = self.inner.clone();
        let historical_client = self.historical_client.clone();
        let provider = self.provider.clone();
        let bedrock_block = self.bedrock_block;

        Box::pin(async move {
            let maybe_block_id = match method_owned.as_str() {
                "eth_getBlockByNumber" | "eth_getBlockByHash" => {
                    parse_block_id_from_params(&params_owned, 0).ok()
                }

                "eth_getBalance"
                | "eth_getStorageAt"
                | "eth_getCode"
                | "eth_getTransactionCount"
                | "eth_call" => parse_block_id_from_params(&params_owned, 1).ok(),

                "eth_getTransactionByHash" | "eth_getTransactionReceipt" => {
                    get_block_id_from_tx_hash(&provider, &params_owned)
                }

                _ => None,
            };

            if let Some(block_id) = maybe_block_id {
                let is_pre_bedrock = match &block_id {
                    BlockId::Number(BlockNumberOrTag::Number(n)) => *n < bedrock_block,
                    BlockId::Number(BlockNumberOrTag::Earliest) => true,
                    BlockId::Number(_) => false,
                    BlockId::Hash(h) => provider
                        .sealed_header_by_hash((*h).into())
                        .ok()
                        .flatten()
                        .map_or(true, |hdr| hdr.header().number() < bedrock_block),
                };

                if is_pre_bedrock {
                    tracing::debug!(target: "rpc::historical",
                                method = %method_owned, ?block_id,
                                "forwarding pre-Bedrock request");

                    let client_params = params_as_vec(&params_owned);

                    if let Ok(resp_json) = historical_client
                        .request::<JsonValue, _>(method_owned.as_str(), client_params)
                        .await
                    {
                        let raw: Box<RawValue> = RawValue::from_string(
                            serde_json::to_string(&resp_json).expect("hist node sent invalid JSON"),
                        )
                        .expect("string is valid JSON");

                        let payload = jsonrpsee_types::ResponsePayload::success(raw).into();

                        return MethodResponse::response(id_owned, payload, usize::MAX);
                    }

                    tracing::warn!(target: "rpc::historical",
                               method = %method_owned, ?block_id,
                               "historical request failed, falling back");
                }
            }

            let params_box =
                RawValue::from_string(params_owned.as_str().unwrap_or("[]").to_owned()).ok();

            let fallback_req = Request::owned(method_owned, params_box, id_owned);

            inner_service.call(fallback_req).await
        })
    }

    fn batch<'a>(&self, req: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        self.inner.batch(req)
    }

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        self.inner.notification(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::{BlockId, BlockNumberOrTag};
    use jsonrpsee::types::Params;
    /// Tests that various valid BlockId types can be parsed from the first parameter.
    #[test]
    fn parses_block_id_from_first_param() {
        // Test with a block number
        let params_num = Params::new(Some(r#"["0x64"]"#)); // 100
        assert_eq!(
            parse_block_id_from_params(&params_num, 0).unwrap(),
            BlockId::Number(BlockNumberOrTag::Number(100))
        );

        // Test with the "earliest" tag
        let params_tag = Params::new(Some(r#"["earliest"]"#));
        assert_eq!(
            parse_block_id_from_params(&params_tag, 0).unwrap(),
            BlockId::Number(BlockNumberOrTag::Earliest)
        );
    }

    /// Tests that the function correctly parses from a position other than 0.
    #[test]
    fn parses_block_id_from_second_param() {
        let params =
            Params::new(Some(r#"["0x0000000000000000000000000000000000000000", "latest"]"#));
        let result = parse_block_id_from_params(&params, 1).unwrap();
        assert_eq!(result, BlockId::Number(BlockNumberOrTag::Latest));
    }

    /// Tests that the function defaults to "latest" if the parameter is missing.
    #[test]
    fn defaults_to_latest_when_param_is_missing() {
        let params = Params::new(Some(r#"["0x0000000000000000000000000000000000000000"]"#));
        let result = parse_block_id_from_params(&params, 1).unwrap();
        assert_eq!(result, BlockId::Number(BlockNumberOrTag::Latest));
    }

    /// Tests that the function returns an error for invalid input.
    #[test]
    fn returns_error_for_invalid_input() {
        let params = Params::new(Some(r#"[true]"#));
        let result = parse_block_id_from_params(&params, 0);
        assert!(result.is_err());
    }
}
