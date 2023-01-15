//! Support for metering senders. Facilitates debugging by exposing metrics for number of messages
//! sent, number of errors, etc.

use metrics::Counter;
use reth_metrics_derive::Metrics;
use tokio::sync::mpsc::{
    error::{SendError, TrySendError},
    Sender,
};

/// Network throughput metrics
#[derive(Metrics)]
#[metrics(dynamic = true)]
struct MeteredSenderMetrics {
    /// Number of messages sent
    messages_sent: Counter,
    /// Number of failed message deliveries
    send_errors: Counter,
}

/// Manages updating the network throughput metrics for a metered stream
pub struct MeteredSender<T> {
    /// The [`Sender`] that this wraps around
    sender: Sender<T>,
    /// Holds the gauges for inbound and outbound throughput
    metrics: MeteredSenderMetrics,
}

impl<T> MeteredSender<T> {
    /// Creates a new [`MeteredSender`] wrapping around the provided [`Sender`]
    pub fn new(sender: Sender<T>, scope: &'static str) -> Self {
        Self { sender, metrics: MeteredSenderMetrics::new(scope) }
    }

    /// Calls the underlying [`Sender`]'s `try_send`, incrementing the appropriate
    /// metrics depending on the result.
    pub fn try_send(&mut self, message: T) -> Result<(), TrySendError<T>> {
        match self.sender.try_send(message) {
            Ok(()) => {
                self.metrics.messages_sent.increment(1);
                Ok(())
            }
            Err(error) => {
                self.metrics.send_errors.increment(1);
                Err(error)
            }
        }
    }

    /// Calls the underlying [`Sender`]'s `send`, incrementing the appropriate
    /// metrics depending on the result.
    pub async fn send(&mut self, value: T) -> Result<(), SendError<T>> {
        match self.sender.send(value).await {
            Ok(()) => {
                self.metrics.messages_sent.increment(1);
                Ok(())
            }
            Err(error) => {
                self.metrics.send_errors.increment(1);
                Err(error)
            }
        }
    }
}
