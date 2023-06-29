use crate::listener::events::{SyncMetricEvent, SyncMetrics};
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Alias type for metric producers to use.
pub type MetricEventsSender = UnboundedSender<MetricEvent>;

/// Collection of metrics.
#[derive(Clone, Copy, Debug)]
pub enum MetricEvent {
    /// Pipeline and live sync.
    Sync(SyncMetricEvent),
}

/// Metrics routine that listens to new metric events on the `events_rx` receiver.
#[derive(Debug)]
pub struct MetricsListener {
    events_rx: UnboundedReceiver<MetricEvent>,
    pub(crate) sync_metrics: SyncMetrics,
}

impl MetricsListener {
    /// Creates a new [MetricsListener] with the provided receiver of [MetricEvent].
    pub fn new(events_rx: UnboundedReceiver<MetricEvent>) -> Self {
        Self { events_rx, sync_metrics: SyncMetrics::default() }
    }

    /// Umbrella handler for all events. Forwards the event to a dedicated handler.  
    fn handle_event(&mut self, event: MetricEvent) {
        match event {
            MetricEvent::Sync(event) => self.handle_sync_event(event),
        }
    }
}

impl Future for MetricsListener {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(event) = ready!(this.events_rx.poll_recv(cx)) else {
            return Poll::Ready(())
        };

        this.handle_event(event);

        Poll::Pending
    }
}
