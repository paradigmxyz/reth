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

/// Metrics for the RPC server.
///
/// Metrics are divided into two categories:
/// - Connection metrics: metrics for the connection (e.g. number of connections opened, relevant
///   for WS and IPC)
/// - Call metrics: metrics for each RPC method (e.g. number of calls started, time taken to process
///   a call)
#[derive(Default, Clone)]
pub(crate) struct RpcServerMetrics {
    inner: Arc<RpcServerMetricsInner>,
}

impl RpcServerMetrics {
    pub(crate) fn new(module: &RpcModule<()>) -> Self {
        Self {
            inner: Arc::new(RpcServerMetricsInner {
                connection_metrics: transport_protocol.connection_metrics(),
                call_metrics: HashMap::from_iter(module.method_names().map(|method| {
                    (method, RpcServerCallMetrics::new_with_labels(&[("method", method)]))
                })),
            }),
        }
    }
}

/// A [RpcServiceT] middleware that captures RPC metrics for the server.
#[derive(Default, Clone)]
pub(crate) struct RpcServerMetricsLayer<S> {
    metrics: RpcServerMetrics,
    service: S,
}

impl<S> RpcServerMetricsLayer<S> {
    pub(crate) fn new(service: S, metrics: RpcServerMetrics) -> Self {
        Self { service, metrics }
    }
}

// TODO: only do this for Call metrics
impl<'a, S> RpcServiceT<'a> for RpcServerMetricsLayer<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = MeteredRequestFuture<S::Future>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        self.metrics.inner.connection_metrics.requests_started.increment(1);
        let call_metrics = self.metrics.inner.call_metrics.get_key_value(req.method.as_ref());
        if let Some((_, call_metrics)) = &call_metrics {
            call_metrics.started.increment(1);
        }
        MeteredRequestFuture {
            fut: self.service.call(req),
            started_at: Instant::now(),
            metrics: self.metrics.clone(),
            method: call_metrics.map(|(method, _)| *method),
        }
    }
}

/// Metrics for the RPC server
#[derive(Default, Clone)]
struct RpcServerMetricsInner {
    /// Connection metrics per transport type
    connection_metrics: RpcServerConnectionMetrics,
    /// Call metrics per RPC method
    call_metrics: HashMap<&'static str, RpcServerCallMetrics>,
}

/// Response future to update the metrics for a single request/response pair.
#[pin_project::pin_project]
pub(crate) struct MeteredRequestFuture<F> {
    #[pin]
    fut: F,
    /// time when the request started
    started_at: Instant,
    /// metrics for the method call
    metrics: RpcServerMetrics,
    /// the method name if known
    method: Option<&'static str>,
}

impl<F> MeteredRequestFuture<F> {
    /// Returns the call metrics for the method if known.
    fn call_metrics(&self) -> Option<&RpcServerCallMetrics> {
        self.metrics.inner.call_metrics.get(self.method?)
    }
}

impl<F> std::fmt::Debug for MeteredRequestFuture<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MeteredRequestFuture")
    }
}

impl<F: Future<Output = MethodResponse>> Future for MeteredRequestFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let fut = self.project().fut;

        let res = fut.poll(cx);
        if let Poll::Ready(resp) = &res {
            let elapsed = self.started_at.elapsed().as_secs_f64();

            // update transport metrics
            self.metrics.inner.connection_metrics.requests_finished.increment(1);
            self.metrics.inner.connection_metrics.request_time_seconds.record(elapsed);

            // update call metrics
            if let Some(call_metrics) = self.call_metrics() {
                call_metrics.time_seconds.record(elapsed);
                if resp.is_success() {
                    call_metrics.successful.increment(1);
                } else {
                    call_metrics.failed.increment(1);
                }
            }
        }
        res
    }
}

/// The transport protocol used for the RPC connection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum TransportProtocol {
    Http,
    WebSocket,
    Ipc,
}

impl TransportProtocol {
    /// Returns the string representation of the transport protocol.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            TransportProtocol::Http => "http",
            TransportProtocol::WebSocket => "ws",
            TransportProtocol::Ipc => "ipc",
        }
    }

    /// Returns the connection metrics for the transport protocol.
    pub(crate) fn connection_metrics(&self) -> RpcServerConnectionMetrics {
        RpcServerConnectionMetrics::new_with_labels(&[("transport", self.as_str())])
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

// impl Logger for RpcServerMetrics {
//     type Instant = Instant;
//
//     fn on_connect(
//         &self,
//         _remote_addr: SocketAddr,
//         _request: &HttpRequest,
//         transport: TransportProtocol,
//     ) {
//         self.inner.connection_metrics.get_metrics(transport).connections_opened.increment(1)
//     }
//
//     fn on_request(&self, transport: TransportProtocol) -> Self::Instant {
//         self.inner.connection_metrics.get_metrics(transport).requests_started.increment(1);
//         Instant::now()
//     }
//
//     fn on_call(
//         &self,
//         method_name: &str,
//         _params: Params<'_>,
//         _kind: MethodKind,
//         _transport: TransportProtocol,
//     ) {
//         let Some(call_metrics) = self.inner.call_metrics.get(method_name) else { return };
//         call_metrics.started.increment(1);
//     }
//
//     fn on_result(
//         &self,
//         method_name: &str,
//         success: MethodResponseResult,
//         started_at: Self::Instant,
//         _transport: TransportProtocol,
//     ) {
//         let Some(call_metrics) = self.inner.call_metrics.get(method_name) else { return };
//
//         // capture call latency
//         call_metrics.time_seconds.record(started_at.elapsed().as_secs_f64());
//         if success.is_success() {
//             call_metrics.successful.increment(1);
//         } else {
//             call_metrics.failed.increment(1);
//         }
//     }
//
//     fn on_response(&self, _result: &str, started_at: Self::Instant, transport: TransportProtocol)
// {         let metrics = self.inner.connection_metrics.get_metrics(transport);
//         // capture request latency for this request/response pair
//         metrics.request_time_seconds.record(started_at.elapsed().as_secs_f64());
//         metrics.requests_finished.increment(1);
//     }
//
//     fn on_disconnect(&self, _remote_addr: SocketAddr, transport: TransportProtocol) {
//         self.inner.connection_metrics.get_metrics(transport).connections_closed.increment(1)
//     }
// }
