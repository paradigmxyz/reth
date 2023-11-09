use jsonrpsee::{
    helpers::MethodResponseResult,
    server::logger::{HttpRequest, Logger, MethodKind, Params, TransportProtocol},
    RpcModule,
};
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Instant};

/// Metrics for the RPC server
#[derive(Default, Clone)]
pub(crate) struct RpcServerMetrics {
    inner: Arc<RpcServerMetricsInner>,
}

/// Metrics for the RPC server
#[derive(Default, Clone)]
struct RpcServerMetricsInner {
    /// Connection metrics per transport type
    connection_metrics: ConnectionMetrics,
    /// Call metrics per RPC method
    call_metrics: HashMap<&'static str, RpcServerCallMetrics>,
}

impl RpcServerMetrics {
    pub(crate) fn new(module: &RpcModule<()>) -> Self {
        Self {
            inner: Arc::new(RpcServerMetricsInner {
                connection_metrics: ConnectionMetrics::default(),
                call_metrics: HashMap::from_iter(module.method_names().map(|method| {
                    (method, RpcServerCallMetrics::new_with_labels(&[("method", method)]))
                })),
            }),
        }
    }
}

#[derive(Clone)]
struct ConnectionMetrics {
    http: RpcServerConnectionMetrics,
    ws: RpcServerConnectionMetrics,
}

impl ConnectionMetrics {
    fn get_metrics(&self, transport: TransportProtocol) -> &RpcServerConnectionMetrics {
        match transport {
            TransportProtocol::Http => &self.http,
            TransportProtocol::WebSocket => &self.ws,
        }
    }
}

impl Default for ConnectionMetrics {
    fn default() -> Self {
        Self {
            http: RpcServerConnectionMetrics::new_with_labels(&[("transport", "http")]),
            ws: RpcServerConnectionMetrics::new_with_labels(&[("transport", "ws")]),
        }
    }
}

/// Metrics for the RPC connections
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server.connections")]
struct RpcServerConnectionMetrics {
    /// The number of connections opened
    connections_opened: Counter,
    /// The number of connections closed
    connections_closed: Counter,
    /// The number of requests started
    requests_started: Counter,
    /// The number of requests finished
    requests_finished: Counter,
    /// Response for a single request/response pair
    request_time_seconds: Histogram,
}

/// Metrics for the RPC calls
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server.calls")]
struct RpcServerCallMetrics {
    /// The number of calls started
    started: Counter,
    /// The number of successful calls
    successful: Counter,
    /// The number of failed calls
    failed: Counter,
    /// Response for a single call
    time_seconds: Histogram,
}

impl Logger for RpcServerMetrics {
    type Instant = Instant;

    fn on_connect(
        &self,
        _remote_addr: SocketAddr,
        _request: &HttpRequest,
        transport: TransportProtocol,
    ) {
        self.inner.connection_metrics.get_metrics(transport).connections_opened.increment(1)
    }

    fn on_request(&self, transport: TransportProtocol) -> Self::Instant {
        self.inner.connection_metrics.get_metrics(transport).requests_started.increment(1);
        Instant::now()
    }

    fn on_call(
        &self,
        method_name: &str,
        _params: Params<'_>,
        _kind: MethodKind,
        _transport: TransportProtocol,
    ) {
        let Some(call_metrics) = self.inner.call_metrics.get(method_name) else { return };
        call_metrics.started.increment(1);
    }

    fn on_result(
        &self,
        method_name: &str,
        success: MethodResponseResult,
        started_at: Self::Instant,
        _transport: TransportProtocol,
    ) {
        let Some(call_metrics) = self.inner.call_metrics.get(method_name) else { return };

        // capture call latency
        call_metrics.time_seconds.record(started_at.elapsed().as_secs_f64());
        if success.is_success() {
            call_metrics.successful.increment(1);
        } else {
            call_metrics.failed.increment(1);
        }
    }

    fn on_response(&self, _result: &str, started_at: Self::Instant, transport: TransportProtocol) {
        let metrics = self.inner.connection_metrics.get_metrics(transport);
        // capture request latency for this request/response pair
        metrics.request_time_seconds.record(started_at.elapsed().as_secs_f64());
        metrics.requests_finished.increment(1);
    }

    fn on_disconnect(&self, _remote_addr: SocketAddr, transport: TransportProtocol) {
        self.inner.connection_metrics.get_metrics(transport).connections_closed.increment(1)
    }
}
