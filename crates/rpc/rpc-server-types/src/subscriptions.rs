//! Tracking of active JSON-RPC subscriptions.
//!
//! The RPC server middleware creates one [`SubscriptionTracker`] per transport and one
//! [`ConnectionHandle`] per connection, and inserts a [`SubscriptionContext`] into the extensions
//! of every subscribe request. Subscription handlers opt in by wrapping the task that owns the
//! accepted sink with [`track_subscription`], which counts the subscription as active until the
//! task completes or is dropped. This covers every way a subscription can end: unsubscribe, the
//! handler returning, and the connection closing.

use jsonrpsee_types::Extensions;
use parking_lot::Mutex;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

/// Tracks active subscriptions of a single transport.
#[derive(Debug)]
pub struct SubscriptionTracker {
    /// Metrics labeled with the transport.
    metrics: SubscriptionTransportMetrics,
    /// Number of connections at each non-zero active subscription count.
    ///
    /// The last key is the current per-connection high watermark. All per-connection count
    /// transitions happen under this lock, so the map never disagrees with the counters.
    levels: Mutex<BTreeMap<u32, u32>>,
}

impl SubscriptionTracker {
    /// Creates a new tracker for the given transport label.
    pub fn new(transport: &'static str) -> Arc<Self> {
        Arc::new(Self {
            metrics: SubscriptionTransportMetrics::new_with_labels(&[("transport", transport)]),
            levels: Default::default(),
        })
    }

    /// Creates the subscription state for a newly opened connection.
    pub fn connection(self: &Arc<Self>) -> ConnectionHandle {
        ConnectionHandle(Arc::new(ConnectionCounts {
            tracker: self.clone(),
            active: AtomicU32::new(0),
            peak: AtomicU32::new(0),
        }))
    }

    /// Records a subscription rejected by the per-connection subscription limit.
    pub fn on_rejected(&self) {
        self.metrics.rejected_total.increment(1);
    }

    /// Returns the number of currently active subscriptions.
    pub fn active(&self) -> u64 {
        self.levels.lock().iter().map(|(level, conns)| u64::from(*level) * u64::from(*conns)).sum()
    }

    /// Returns the largest active subscription count on any single connection.
    pub fn max_per_connection(&self) -> u32 {
        Self::max_level(&self.levels.lock())
    }

    fn on_open(&self, conn: &ConnectionCounts) {
        let mut levels = self.levels.lock();
        let to = conn.active.fetch_add(1, Ordering::Relaxed) + 1;
        conn.peak.fetch_max(to, Ordering::Relaxed);
        Self::move_level(&mut levels, to - 1, to);
        self.metrics.max_per_connection.set(Self::max_level(&levels));
        self.metrics.opened_total.increment(1);
        self.metrics.active.increment(1);
    }

    fn on_close(&self, conn: &ConnectionCounts) {
        let mut levels = self.levels.lock();
        let from = conn.active.fetch_sub(1, Ordering::Relaxed);
        Self::move_level(&mut levels, from, from - 1);
        self.metrics.max_per_connection.set(Self::max_level(&levels));
        self.metrics.closed_total.increment(1);
        self.metrics.active.decrement(1);
    }

    fn move_level(levels: &mut BTreeMap<u32, u32>, from: u32, to: u32) {
        if from > 0 &&
            let Some(conns) = levels.get_mut(&from)
        {
            *conns -= 1;
            if *conns == 0 {
                levels.remove(&from);
            }
        }
        if to > 0 {
            *levels.entry(to).or_default() += 1;
        }
    }

    fn max_level(levels: &BTreeMap<u32, u32>) -> u32 {
        levels.last_key_value().map_or(0, |(level, _)| *level)
    }
}

/// Subscription state of a connection, owned by the RPC middleware.
///
/// Dropping the handle means the connection closed.
#[derive(Debug)]
pub struct ConnectionHandle(Arc<ConnectionCounts>);

impl ConnectionHandle {
    /// Returns the context to insert into the extensions of a subscribe request for `method`.
    pub fn context(&self, method: &'static str) -> SubscriptionContext {
        SubscriptionContext { connection: self.0.clone(), method }
    }
}

impl Drop for ConnectionHandle {
    fn drop(&mut self) {
        // No subscribe calls can arrive after the connection closed, so the peak is final.
        let peak = self.0.peak.load(Ordering::Relaxed);
        if peak > 0 {
            self.0.tracker.metrics.connection_peak.record(peak);
        }
    }
}

/// Context inserted by the RPC middleware into the extensions of subscribe requests.
#[derive(Debug, Clone)]
pub struct SubscriptionContext {
    /// Subscription counts of the connection that sent the request.
    connection: Arc<ConnectionCounts>,
    /// The subscribe method name.
    method: &'static str,
}

/// Counts `fut` as an active subscription until it completes or is dropped.
///
/// Call this after `PendingSubscriptionSink::accept` succeeds and spawn the returned future in
/// place of `fut`. `kind` is an optional bounded label that distinguishes subscriptions of the
/// same method, e.g. the `eth_subscribe` kind; pass `""` if there is none.
///
/// Does nothing if the server doesn't track subscriptions, i.e. `ext` contains no
/// [`SubscriptionContext`].
pub fn track_subscription<F: Future>(
    ext: &Extensions,
    kind: &'static str,
    fut: F,
) -> impl Future<Output = F::Output> + use<F> {
    // Create the guard eagerly so the subscription counts even before the task is first polled.
    let guard = ext.get::<SubscriptionContext>().map(|ctx| SubscriptionGuard::new(ctx, kind));
    async move {
        let _guard = guard;
        fut.await
    }
}

/// Subscription counts of a single connection.
#[derive(Debug)]
struct ConnectionCounts {
    tracker: Arc<SubscriptionTracker>,
    active: AtomicU32,
    peak: AtomicU32,
}

/// Counts one accepted subscription for as long as it's alive.
#[derive(Debug)]
struct SubscriptionGuard {
    connection: Arc<ConnectionCounts>,
    method: SubscriptionMethodMetrics,
}

impl SubscriptionGuard {
    fn new(ctx: &SubscriptionContext, kind: &'static str) -> Self {
        let method =
            SubscriptionMethodMetrics::new_with_labels(&[("method", ctx.method), ("kind", kind)]);
        method.method_opened_total.increment(1);
        method.method_active.increment(1);
        ctx.connection.tracker.on_open(&ctx.connection);
        Self { connection: ctx.connection.clone(), method }
    }
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        self.method.method_active.decrement(1);
        self.connection.tracker.on_close(&self.connection);
    }
}

/// Subscription metrics per transport.
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server.subscriptions")]
struct SubscriptionTransportMetrics {
    /// The number of subscriptions opened
    opened_total: Counter,
    /// The number of subscriptions closed
    closed_total: Counter,
    /// The number of currently active subscriptions
    active: Gauge,
    /// The number of subscriptions rejected by the per-connection subscription limit
    rejected_total: Counter,
    /// The largest active subscription count on any single connection
    max_per_connection: Gauge,
    /// The peak active subscription count of each closed connection that subscribed at least once
    connection_peak: Histogram,
}

/// Subscription metrics per subscribe method and subscription kind.
#[derive(Metrics, Clone)]
#[metrics(scope = "rpc_server.subscriptions")]
struct SubscriptionMethodMetrics {
    /// The number of subscriptions opened
    method_opened_total: Counter,
    /// The number of currently active subscriptions
    method_active: Gauge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};

    #[test]
    fn tracks_active_and_high_watermark() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            let tracker = SubscriptionTracker::new("ws");
            let conn_a = tracker.connection();
            let conn_b = tracker.connection();
            let idle = tracker.connection();

            let a1 = SubscriptionGuard::new(&conn_a.context("eth_subscribe"), "newHeads");
            let a2 = SubscriptionGuard::new(&conn_a.context("eth_subscribe"), "logs");
            let b1 = SubscriptionGuard::new(&conn_b.context("eth_subscribe"), "logs");
            assert_eq!(tracker.active(), 3);
            assert_eq!(tracker.max_per_connection(), 2);

            drop(a1);
            assert_eq!(tracker.active(), 2);
            assert_eq!(tracker.max_per_connection(), 1);

            // A connection that never subscribed doesn't record a peak, and a closed connection
            // records its peak once, even while one of its subscriptions is still alive.
            drop(idle);
            drop(conn_a);
            assert_eq!(tracker.active(), 2);

            drop(a2);
            drop(b1);
            drop(conn_b);
            assert_eq!(tracker.active(), 0);
            assert_eq!(tracker.max_per_connection(), 0);
            tracker.on_rejected();
        });

        // Snapshots reset values, so take a single one at the end.
        let snapshot = snapshotter.snapshot().into_vec();
        let value = |name: &str, labels: &[(&str, &str)]| {
            snapshot.iter().find_map(|(key, _, _, value)| {
                let key = key.key();
                (key.name() == format!("rpc_server.subscriptions.{name}") &&
                    labels
                        .iter()
                        .all(|(k, v)| key.labels().any(|l| l.key() == *k && l.value() == *v)))
                .then_some(value)
            })
        };

        let ws = [("transport", "ws")];
        assert_eq!(value("opened_total", &ws), Some(&DebugValue::Counter(3)));
        assert_eq!(value("closed_total", &ws), Some(&DebugValue::Counter(3)));
        assert_eq!(value("rejected_total", &ws), Some(&DebugValue::Counter(1)));
        assert_eq!(value("active", &ws), Some(&DebugValue::Gauge(0.0.into())));
        assert_eq!(value("max_per_connection", &ws), Some(&DebugValue::Gauge(0.0.into())));
        assert_eq!(
            value("connection_peak", &ws),
            Some(&DebugValue::Histogram(vec![2.0.into(), 1.0.into()]))
        );

        let logs = [("method", "eth_subscribe"), ("kind", "logs")];
        assert_eq!(value("method_opened_total", &logs), Some(&DebugValue::Counter(2)));
        assert_eq!(value("method_active", &logs), Some(&DebugValue::Gauge(0.0.into())));
        let new_heads = [("method", "eth_subscribe"), ("kind", "newHeads")];
        assert_eq!(value("method_opened_total", &new_heads), Some(&DebugValue::Counter(1)));
    }
}
