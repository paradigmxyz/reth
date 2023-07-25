use jsonrpsee::server::logger::Logger;
use jsonrpsee::server::logger::{HttpRequest, MethodKind, Params, TransportProtocol};
use std::format;
use std::net::SocketAddr;

use reth_metrics::{
    metrics::{self, counter, histogram, Gauge, Histogram},
    Metrics,
};
use std::time::Instant;

const METRICS_SCOPE: &str = "rpc_server";
const CALL_LATENCY_METRIC: &str = "call_latency";
const CALL_COUNT_METRIC: &str = "call_count";
const CALL_ERROR_METRIC: &str = "call_error";

/// Metrics for the rpc server
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server")]
pub(crate) struct RpcServerMetrics {
    /// The number of ws requests currently being served
    ws_req_count: Gauge,
    /// The number of http requests currently being served
    http_req_count: Gauge,
    /// The number of ws sessions currently active
    ws_session_count: Gauge,
    /// The number of http connections currently active
    http_session_count: Gauge,
    /// Latency for a single request/response pair
    request_latency: Histogram,
}

impl Logger for RpcServerMetrics {
    type Instant = Instant;
    fn on_connect(
        &self,
        _remote_addr: SocketAddr,
        _request: &HttpRequest,
        transport: TransportProtocol,
    ) {
        match transport {
            TransportProtocol::Http => self.http_session_count.increment(1 as f64),
            TransportProtocol::WebSocket => self.ws_session_count.increment(1 as f64),
        }
    }
    fn on_request(&self, transport: TransportProtocol) -> Self::Instant {
        match transport {
            TransportProtocol::Http => self.http_req_count.increment(1 as f64),
            TransportProtocol::WebSocket => self.ws_req_count.increment(1 as f64),
        }
        Instant::now()
    }
    fn on_call(
        &self,
        method_name: &str,
        _params: Params<'_>,
        _kind: MethodKind,
        _transport: TransportProtocol,
    ) {
        // note that on_call will be called multiple times in case of a batch request
        // increment method call count, use macro because derive(Metrics) doesnt seem to support dynamically configuring metric name (?)
        let metric_call_count_name =
            format!("{}{}{}{}{}", METRICS_SCOPE, "_", CALL_COUNT_METRIC, "_", method_name);
        counter!(metric_call_count_name, 1); // this could be a gauge since one call here should map to one "result" in on_result
    }
    fn on_result(
        &self,
        method_name: &str,
        success: bool,
        started_at: Self::Instant,
        _transport: TransportProtocol,
    ) {
        // capture method call latency, use macro because of the same reason stated in on_call
        let metric_name_call_latency =
            format!("{}{}{}{}{}", METRICS_SCOPE, "_", CALL_LATENCY_METRIC, "_", method_name);
        histogram!(metric_name_call_latency, started_at.elapsed());
        if !success {
            // capture error count for method call, use macro because of the same reason stated in on_call
            let metric_name_call_error_count =
                format!("{}{}{}{}{}", METRICS_SCOPE, "_", CALL_ERROR_METRIC, "_", method_name);
            counter!(metric_name_call_error_count, 1);
        }
    }
    fn on_response(&self, _result: &str, started_at: Self::Instant, transport: TransportProtocol) {
        match transport {
            TransportProtocol::Http => self.http_req_count.decrement(1 as f64),
            TransportProtocol::WebSocket => self.ws_req_count.decrement(1 as f64),
        }
        // capture request latency for this request/response pair
        self.request_latency.record(started_at.elapsed());
    }
    fn on_disconnect(&self, _remote_addr: SocketAddr, transport: TransportProtocol) {
        match transport {
            TransportProtocol::Http => self.http_session_count.decrement(1 as f64),
            TransportProtocol::WebSocket => self.ws_session_count.decrement(1 as f64),
        }
    }
}
