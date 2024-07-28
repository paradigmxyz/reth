use jsonrpsee::{server::middleware::rpc::RpcServiceT, types::Request, MethodResponse, RpcModule};
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
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
                call_metrics: HashMap::from_iter(module.method_names().map(|method| {
                    (method, RpcServerCallMetrics::new_with_labels(&[("method", method)]))
                })),
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
    #[allow(unused)]
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

impl<'a, S> RpcServiceT<'a> for RpcRequestMetricsService<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = MeteredRequestFuture<S::Future>;

    fn call(&self, req: Request<'a>) -> Self::Future {
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
    #[allow(unused)]
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
