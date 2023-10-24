use crate::metrics::{Metrics, Operation, TransactionMode, TransactionOutcome};
use reth_tracing::tracing::trace;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Alias type for metric producers to use.
pub type MetricEventsSender = UnboundedSender<MetricEvent>;

/// Collection of metric events.
#[derive(Clone, Copy, Debug)]
#[allow(missing_docs)]
pub enum MetricEvent {
    OpenTransaction { txn_id: u64, mode: TransactionMode },
    CloseTransaction { txn_id: u64, outcome: TransactionOutcome, close_duration: Duration },
    Operation { operation: Operation, duration: Duration },
}

/// Metrics routine that listens to new metric events on the `events_rx` receiver.
/// Upon receiving new event, related metrics are updated.
#[derive(Debug)]
pub struct MetricsListener {
    events_rx: UnboundedReceiver<MetricEvent>,
    metrics: Metrics,
}

impl MetricsListener {
    /// Creates a new [MetricsListener] with the provided receiver of [MetricEvent].
    pub fn new(events_rx: UnboundedReceiver<MetricEvent>) -> Self {
        Self { events_rx, metrics: Metrics::default() }
    }

    fn handle_event(&mut self, event: MetricEvent) {
        trace!(target: "storage::metrics", ?event, "Metric event received");
        match event {
            MetricEvent::OpenTransaction { txn_id, mode } => {
                self.metrics.record_open_transaction(txn_id, mode)
            }
            MetricEvent::CloseTransaction { txn_id, outcome, close_duration } => {
                self.metrics.record_close_transaction(txn_id, outcome, close_duration)
            }
            MetricEvent::Operation { operation, duration } => {
                self.metrics.record_operation(operation, duration)
            }
        }
    }
}

impl Future for MetricsListener {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Loop until we drain the `events_rx` channel
        loop {
            let Some(event) = ready!(this.events_rx.poll_recv(cx)) else {
                // Channel has closed
                return Poll::Ready(())
            };

            this.handle_event(event);
        }
    }
}
