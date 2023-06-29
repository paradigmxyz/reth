use crate::routine::events::{SyncMetricEvent, SyncMetrics};
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub type MetricEventsSender = UnboundedSender<MetricEvent>;

pub enum MetricEvent {
    Sync(SyncMetricEvent),
}

pub struct MetricsListener {
    events_tx: UnboundedReceiver<MetricEvent>,
    pub(crate) sync_metrics: SyncMetrics,
}

impl MetricsListener {
    pub fn new(events_tx: UnboundedReceiver<MetricEvent>) -> Self {
        Self { events_tx, sync_metrics: SyncMetrics::default() }
    }

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
        let Some(event) = ready!(this.events_tx.poll_recv(cx)) else {
            return Poll::Ready(())
        };

        this.handle_event(event);

        Poll::Pending
    }
}
