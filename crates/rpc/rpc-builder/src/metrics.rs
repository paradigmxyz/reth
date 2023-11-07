use jsonrpsee::{
    helpers::MethodResponseResult,
    server::logger::{HttpRequest, Logger, MethodKind, Params, TransportProtocol},
};
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use std::{net::SocketAddr, time::Instant};

/// Metrics for the rpc server
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server")]
pub(crate) struct RpcServerMetrics {
    /// The number of requests started
    requests_started: Counter,
    /// The number of requests finished
    requests_finished: Counter,
    /// The number of ws sessions opened
    ws_session_opened: Counter,
    /// The number of ws sessions closed
    ws_session_closed: Counter,
    /// Latency for a single request/response pair
    request_latency: Histogram,
}

#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server.call")]
struct RpcServerCallMetrics {
    /// The number of calls started
    calls_started: Counter,
    /// The number of successful calls
    successful_calls: Counter,
    /// The number of failed calls
    failed_calls: Counter,
    /// Latency for a single call
    call_latency: Histogram,
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
            TransportProtocol::Http => {}
            TransportProtocol::WebSocket => self.ws_session_opened.increment(1),
        }
    }
    fn on_request(&self, _transport: TransportProtocol) -> Self::Instant {
        self.requests_started.increment(1);
        Instant::now()
    }
    fn on_call(
        &self,
        method_name: &str,
        _params: Params<'_>,
        _kind: MethodKind,
        _transport: TransportProtocol,
    ) {
        let call_metrics =
            RpcServerCallMetrics::new_with_labels(&[("method", method_name.to_string())]);
        call_metrics.calls_started.increment(1);
    }
    fn on_result(
        &self,
        method_name: &str,
        success: MethodResponseResult,
        started_at: Self::Instant,
        _transport: TransportProtocol,
    ) {
        let call_metrics =
            RpcServerCallMetrics::new_with_labels(&[("method", method_name.to_string())]);

        // capture call latency
        call_metrics.call_latency.record(started_at.elapsed().as_millis() as f64);
        if success.is_success() {
            call_metrics.successful_calls.increment(1);
        } else {
            call_metrics.failed_calls.increment(1);
        }
    }
    fn on_response(&self, _result: &str, started_at: Self::Instant, _transport: TransportProtocol) {
        // capture request latency for this request/response pair
        self.request_latency.record(started_at.elapsed().as_millis() as f64);
        self.requests_finished.increment(1);
    }
    fn on_disconnect(&self, _remote_addr: SocketAddr, transport: TransportProtocol) {
        match transport {
            TransportProtocol::Http => {}
            TransportProtocol::WebSocket => self.ws_session_closed.increment(1),
        }
    }
}
