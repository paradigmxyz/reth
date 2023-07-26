use jsonrpsee::server::logger::{HttpRequest, Logger, MethodKind, Params, TransportProtocol};
use std::{format, net::SocketAddr};

use reth_metrics::{
    metrics::{self, counter, histogram, Counter, Gauge, Histogram},
    Metrics,
};
use std::time::Instant;

/// Metrics for the rpc server
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server")]
pub(crate) struct RpcServerMetrics {
    /// The number of calls started
    calls_started: Counter,
    /// The number of successful calls
    successful_calls: Counter,
    /// The number of failed calls
    failed_calls: Counter,
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
    fn on_request(&self, transport: TransportProtocol) -> Self::Instant {
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
        self.calls_started.increment(1);
    }
    fn on_result(
        &self,
        method_name: &str,
        success: bool,
        started_at: Self::Instant,
        _transport: TransportProtocol,
    ) {
        // capture call duration
        self.call_latency.record(started_at.elapsed().as_millis());
        if !success {
            self.failed_calls.increment(1);
        } else {
            self.successful_calls.increment(1);
        }
    }
    fn on_response(&self, _result: &str, started_at: Self::Instant, transport: TransportProtocol) {
        // capture request latency for this request/response pair
        self.request_latency.record(started_at.elapsed().as_millis());
    }
    fn on_disconnect(&self, _remote_addr: SocketAddr, transport: TransportProtocol) {
        match transport {
            TransportProtocol::Http => {}
            TransportProtocol::WebSocket => self.ws_session_closed.increment(1),
        }
    }
}
