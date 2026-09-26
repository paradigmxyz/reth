use jsonrpsee::{
    core::{
        middleware::{Batch, BatchEntry, Notification},
        server::MethodCallback,
    },
    server::middleware::rpc::RpcServiceT,
    types::{error::TOO_MANY_SUBSCRIPTIONS_CODE, Request},
    MethodResponse, RpcModule,
};
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use reth_primitives_traits::FastInstant as Instant;
use reth_rpc_server_types::subscriptions::{ConnectionHandle, SubscriptionTracker};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tower::Layer;

/// Metrics for the RPC server.
///
/// Metrics are divided into two categories:
/// - Connection metrics: metrics for the connection (e.g. number of connections opened, relevant
///   for WS and IPC)
/// - Request metrics: metrics for each RPC method (e.g. number of calls started, time taken to
///   process a call)
///
/// For transports that carry subscriptions, it also tracks active subscriptions, see
/// [`reth_rpc_server_types::subscriptions`].
#[derive(Default, Debug, Clone)]
pub(crate) struct RpcRequestMetrics {
    inner: Arc<RpcServerMetricsInner>,
}

impl RpcRequestMetrics {
    pub(crate) fn new(
        module: &RpcModule<()>,
        transport: RpcTransport,
        carries_subscriptions: bool,
    ) -> Self {
        let subscribe_methods = module
            .method_names()
            .filter(|method| matches!(module.method(method), Some(MethodCallback::Subscription(_))))
            .collect::<HashSet<_>>();
        let subscriptions = (carries_subscriptions && !subscribe_methods.is_empty())
            .then(|| SubscriptionTracker::new(transport.as_str()));
        Self {
            inner: Arc::new(RpcServerMetricsInner {
                connection_metrics: transport.connection_metrics(),
                call_metrics: module
                    .method_names()
                    .map(|method| {
                        (method, RpcServerCallMetrics::new_with_labels(&[("method", method)]))
                    })
                    .collect(),
                subscribe_methods,
                subscriptions,
            }),
        }
    }

    /// Creates a new instance of the metrics layer for HTTP.
    pub(crate) fn http(module: &RpcModule<()>) -> Self {
        Self::new(module, RpcTransport::Http, false)
    }

    /// Creates a new instance of the metrics layer for same port.
    ///
    /// Note: currently it's not possible to track transport specific metrics for a server that runs http and ws on the same port: <https://github.com/paritytech/jsonrpsee/issues/1345> until we have this feature we will use the http metrics for this case.
    pub(crate) fn same_port(module: &RpcModule<()>) -> Self {
        Self::new(module, RpcTransport::Http, true)
    }

    /// Creates a new instance of the metrics layer for Ws.
    pub(crate) fn ws(module: &RpcModule<()>) -> Self {
        Self::new(module, RpcTransport::WebSocket, true)
    }

    /// Creates a new instance of the metrics layer for Ipc.
    pub(crate) fn ipc(module: &RpcModule<()>) -> Self {
        Self::new(module, RpcTransport::Ipc, true)
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
    /// Names of the subscribe methods
    subscribe_methods: HashSet<&'static str>,
    /// Subscription tracking, `None` if the transport can't carry subscriptions
    subscriptions: Option<Arc<SubscriptionTracker>>,
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
    /// Subscription state of the connection.
    ///
    /// Shared between clones so that the connection closes exactly once.
    connection: Option<Arc<ConnectionHandle>>,
}

impl<S> RpcRequestMetricsService<S> {
    pub(crate) fn new(service: S, metrics: RpcRequestMetrics) -> Self {
        // this instance is kept alive for the duration of the connection
        metrics.inner.connection_metrics.connections_opened_total.increment(1);
        let connection =
            metrics.inner.subscriptions.as_ref().map(|tracker| Arc::new(tracker.connection()));
        Self { inner: service, metrics, connection }
    }

    /// Inserts the subscription context into the request if it calls a subscribe method.
    ///
    /// Returns whether the request calls a subscribe method.
    fn insert_subscription_context(&self, req: &mut Request<'_>) -> bool {
        let Some(connection) = &self.connection else { return false };
        let Some(method) = self.metrics.inner.subscribe_methods.get(req.method.as_ref()) else {
            return false
        };
        req.extensions_mut().insert(connection.context(method));
        true
    }
}

impl<S> RpcServiceT for RpcRequestMetricsService<S>
where
    S: RpcServiceT<MethodResponse = MethodResponse> + Send + Sync + Clone + 'static,
{
    type MethodResponse = S::MethodResponse;
    type NotificationResponse = S::NotificationResponse;
    type BatchResponse = S::BatchResponse;

    fn call<'a>(
        &self,
        mut req: Request<'a>,
    ) -> impl Future<Output = S::MethodResponse> + Send + 'a {
        self.metrics.inner.connection_metrics.requests_started_total.increment(1);
        let is_subscribe = self.insert_subscription_context(&mut req);
        let call_metrics = self.metrics.inner.call_metrics.get_key_value(req.method.as_ref());
        if let Some((_, call_metrics)) = &call_metrics {
            call_metrics.started_total.increment(1);
        }
        MeteredRequestFuture {
            fut: self.inner.call(req),
            started_at: Instant::now(),
            metrics: self.metrics.clone(),
            method: call_metrics.map(|(method, _)| *method),
            is_subscribe,
        }
    }

    fn batch<'a>(
        &self,
        mut req: Batch<'a>,
    ) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        self.metrics.inner.connection_metrics.batches_started_total.increment(1);

        for batch_entry in req.iter_mut().flatten() {
            let method_name = batch_entry.method_name();
            if let Some(call_metrics) = self.metrics.inner.call_metrics.get(method_name) {
                call_metrics.batched_total.increment(1);
            }
            if let BatchEntry::Call(req) = batch_entry {
                self.insert_subscription_context(req);
            }
        }

        MeteredBatchRequestsFuture {
            fut: self.inner.batch(req),
            started_at: Instant::now(),
            metrics: self.metrics.clone(),
        }
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
    /// whether the request calls a subscribe method
    is_subscribe: bool,
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

            // update subscription metrics
            if *this.is_subscribe &&
                resp.as_error_code() == Some(TOO_MANY_SUBSCRIPTIONS_CODE) &&
                let Some(tracker) = &this.metrics.inner.subscriptions
            {
                tracker.on_rejected();
            }
        }
        res
    }
}

/// Response future to update the metrics for a batch of request/response pairs.
#[pin_project::pin_project]
pub struct MeteredBatchRequestsFuture<F> {
    #[pin]
    fut: F,
    /// time when the batch request started
    started_at: Instant,
    /// metrics for the batch
    metrics: RpcRequestMetrics,
}

impl<F> std::fmt::Debug for MeteredBatchRequestsFuture<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MeteredBatchRequestsFuture")
    }
}

impl<F> Future for MeteredBatchRequestsFuture<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let res = this.fut.poll(cx);

        if res.is_ready() {
            let elapsed = this.started_at.elapsed().as_secs_f64();
            this.metrics.inner.connection_metrics.batches_finished_total.increment(1);
            this.metrics.inner.connection_metrics.batch_response_time_seconds.record(elapsed);
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
    /// The number of batch requests started
    batches_started_total: Counter,
    /// The number of batch requests finished
    batches_finished_total: Counter,
    /// Response time for a batch request
    batch_response_time_seconds: Histogram,
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
    /// The number of calls received as batch entries.
    ///
    /// jsonrpsee dispatches batch entries internally without invoking this middleware's `call`,
    /// so only their per-method volume can be tracked; batched calls are excluded from
    /// `started_total`, `successful_total`, `failed_total` and `time_seconds`.
    batched_total: Counter,
    /// Response for a single call
    time_seconds: Histogram,
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::{
        core::client::{Subscription, SubscriptionClientT},
        rpc_params,
        server::{middleware::rpc::RpcServiceBuilder, Server, ServerConfigBuilder},
        ws_client::{WsClient, WsClientBuilder},
    };
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use reth_rpc_server_types::subscriptions::track_subscription;
    use std::time::Duration;

    fn subscription_module() -> RpcModule<()> {
        let mut module = RpcModule::new(());
        module
            .register_subscription(
                "test_subscribe",
                "test_subscription",
                "test_unsubscribe",
                |_, pending, _, ext| async move {
                    let sink = pending.accept().await?;
                    tokio::spawn(track_subscription(&ext, "", async move { sink.closed().await }));
                    Ok(())
                },
            )
            .unwrap();
        module
    }

    async fn subscribe(
        client: &WsClient,
    ) -> Result<Subscription<()>, jsonrpsee::core::ClientError> {
        client.subscribe("test_subscribe", rpc_params![], "test_unsubscribe").await
    }

    async fn wait_for(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition not reached");
    }

    // Single-threaded so that all server tasks record into the thread-local recorder.
    #[tokio::test(flavor = "current_thread")]
    async fn tracks_ws_subscriptions() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let module = subscription_module();
        let metrics = RpcRequestMetrics::ws(&module);
        let tracker = metrics.inner.subscriptions.clone().unwrap();
        let server = Server::builder()
            .set_config(ServerConfigBuilder::default().max_subscriptions_per_connection(2).build())
            .set_rpc_middleware(RpcServiceBuilder::new().layer(metrics))
            .build("127.0.0.1:0")
            .await
            .unwrap();
        let url = format!("ws://{}", server.local_addr().unwrap());
        let handle = server.start(module);

        let client_a = WsClientBuilder::default().build(&url).await.unwrap();
        let client_b = WsClientBuilder::default().build(&url).await.unwrap();
        let a1 = subscribe(&client_a).await.unwrap();
        let _a2 = subscribe(&client_a).await.unwrap();
        subscribe(&client_a).await.unwrap_err();
        let _b1 = subscribe(&client_b).await.unwrap();
        assert_eq!(tracker.active(), 3);
        assert_eq!(tracker.max_per_connection(), 2);

        a1.unsubscribe().await.unwrap();
        wait_for(|| tracker.active() == 2).await;
        assert_eq!(tracker.max_per_connection(), 1);

        drop(client_b);
        wait_for(|| tracker.active() == 1).await;

        drop(client_a);
        wait_for(|| tracker.active() == 0).await;
        assert_eq!(tracker.max_per_connection(), 0);

        handle.stop().unwrap();
        handle.stopped().await;

        let snapshot = snapshotter.snapshot().into_vec();
        let value = |name: &str| {
            snapshot.iter().find_map(|(key, _, _, value)| {
                (key.key().name() == format!("rpc_server.subscriptions.{name}")).then_some(value)
            })
        };
        assert_eq!(value("opened_total"), Some(&DebugValue::Counter(3)));
        assert_eq!(value("closed_total"), Some(&DebugValue::Counter(3)));
        assert_eq!(value("rejected_total"), Some(&DebugValue::Counter(1)));
        assert_eq!(value("active"), Some(&DebugValue::Gauge(0.0.into())));
        let Some(DebugValue::Histogram(peaks)) = value("connection_peak") else {
            panic!("connection_peak not recorded")
        };
        let mut peaks = peaks.iter().map(|peak| peak.into_inner()).collect::<Vec<_>>();
        peaks.sort_by(f64::total_cmp);
        assert_eq!(peaks, [1.0, 2.0]);
    }
}
