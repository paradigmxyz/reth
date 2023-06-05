//! Support for metering senders. Facilitates debugging by exposing metrics for number of messages
//! sent, number of errors, etc.

use metrics::Counter;
use reth_metrics_derive::Metrics;
use std::task::{ready, Context, Poll};
use tokio::sync::mpsc::{
    self,
    error::{SendError, TryRecvError, TrySendError},
};

/// Wrapper around [mpsc::unbounded_channel] that returns a new unbounded metered channel.
pub fn metered_unbounded_channel<T>(
    scope: &'static str,
) -> (UnboundedMeteredSender<T>, UnboundedMeteredReceiver<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (UnboundedMeteredSender::new(tx, scope), UnboundedMeteredReceiver::new(rx, scope))
}

/// Wrapper around [mpsc::channel] that returns a new bounded metered channel with the given
/// buffer size.
pub fn metered_channel<T>(
    buffer: usize,
    scope: &'static str,
) -> (MeteredSender<T>, MeteredReceiver<T>) {
    let (tx, rx) = mpsc::channel(buffer);
    (MeteredSender::new(tx, scope), MeteredReceiver::new(rx, scope))
}

/// A wrapper type around [UnboundedSender](mpsc::UnboundedSender) that updates metrics on send.
#[derive(Debug)]
pub struct UnboundedMeteredSender<T> {
    /// The [UnboundedSender](mpsc::UnboundedSender) that this wraps around
    sender: mpsc::UnboundedSender<T>,
    /// Holds metrics for this type
    metrics: MeteredSenderMetrics,
}

impl<T> UnboundedMeteredSender<T> {
    /// Creates a new [`MeteredSender`] wrapping around the provided  that updates metrics on send.
    // #[derive(Debug)]
    pub fn new(sender: mpsc::UnboundedSender<T>, scope: &'static str) -> Self {
        Self { sender, metrics: MeteredSenderMetrics::new(scope) }
    }

    /// Calls the underlying  that updates metrics on send.
    // #[derive(Debug)]'s `try_send`, incrementing the appropriate
    /// metrics depending on the result.
    pub fn send(&self, message: T) -> Result<(), SendError<T>> {
        match self.sender.send(message) {
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

impl<T> Clone for UnboundedMeteredSender<T> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone(), metrics: self.metrics.clone() }
    }
}

/// A wrapper type around [Receiver](mpsc::UnboundedReceiver) that updates metrics on receive.
#[derive(Debug)]
pub struct UnboundedMeteredReceiver<T> {
    /// The [Sender](mpsc::Sender) that this wraps around
    receiver: mpsc::UnboundedReceiver<T>,
    /// Holds metrics for this type
    metrics: MeteredReceiverMetrics,
}

// === impl MeteredReceiver ===

impl<T> UnboundedMeteredReceiver<T> {
    /// Creates a new [UnboundedMeteredReceiver] wrapping around the provided
    /// [Receiver](mpsc::UnboundedReceiver)
    pub fn new(receiver: mpsc::UnboundedReceiver<T>, scope: &'static str) -> Self {
        Self { receiver, metrics: MeteredReceiverMetrics::new(scope) }
    }

    /// Receives the next value for this receiver.
    pub async fn recv(&mut self) -> Option<T> {
        let msg = self.receiver.recv().await;
        if msg.is_some() {
            self.metrics.messages_received.increment(1);
        }
        msg
    }

    /// Tries to receive the next value for this receiver.
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let msg = self.receiver.try_recv()?;
        self.metrics.messages_received.increment(1);
        Ok(msg)
    }

    /// Closes the receiving half of a channel without dropping it.
    pub fn close(&mut self) {
        self.receiver.close();
    }

    /// Polls to receive the next message on this channel.
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let msg = ready!(self.receiver.poll_recv(cx));
        if msg.is_some() {
            self.metrics.messages_received.increment(1);
        }
        Poll::Ready(msg)
    }
}

/// A wrapper type around [Sender](mpsc::Sender) that updates metrics on send.
#[derive(Debug)]
pub struct MeteredSender<T> {
    /// The [Sender](mpsc::Sender) that this wraps around
    sender: mpsc::Sender<T>,
    /// Holds metrics for this type
    metrics: MeteredSenderMetrics,
}

impl<T> MeteredSender<T> {
    /// Creates a new [`MeteredSender`] wrapping around the provided [Sender](mpsc::Sender)
    pub fn new(sender: mpsc::Sender<T>, scope: &'static str) -> Self {
        Self { sender, metrics: MeteredSenderMetrics::new(scope) }
    }

    /// Calls the underlying [Sender](mpsc::Sender)'s `try_send`, incrementing the appropriate
    /// metrics depending on the result.
    pub fn try_send(&self, message: T) -> Result<(), TrySendError<T>> {
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

    /// Calls the underlying [Sender](mpsc::Sender)'s `send`, incrementing the appropriate
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

impl<T> Clone for MeteredSender<T> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone(), metrics: self.metrics.clone() }
    }
}

/// A wrapper type around [Receiver](mpsc::Receiver) that updates metrics on receive.
#[derive(Debug)]
pub struct MeteredReceiver<T> {
    /// The [Sender](mpsc::Sender) that this wraps around
    receiver: mpsc::Receiver<T>,
    /// Holds metrics for this type
    metrics: MeteredReceiverMetrics,
}

// === impl MeteredReceiver ===

impl<T> MeteredReceiver<T> {
    /// Creates a new [`MeteredReceiver`] wrapping around the provided [Receiver](mpsc::Receiver)
    pub fn new(receiver: mpsc::Receiver<T>, scope: &'static str) -> Self {
        Self { receiver, metrics: MeteredReceiverMetrics::new(scope) }
    }

    /// Receives the next value for this receiver.
    pub async fn recv(&mut self) -> Option<T> {
        let msg = self.receiver.recv().await;
        if msg.is_some() {
            self.metrics.messages_received.increment(1);
        }
        msg
    }

    /// Tries to receive the next value for this receiver.
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let msg = self.receiver.try_recv()?;
        self.metrics.messages_received.increment(1);
        Ok(msg)
    }

    /// Closes the receiving half of a channel without dropping it.
    pub fn close(&mut self) {
        self.receiver.close();
    }

    /// Polls to receive the next message on this channel.
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let msg = ready!(self.receiver.poll_recv(cx));
        if msg.is_some() {
            self.metrics.messages_received.increment(1);
        }
        Poll::Ready(msg)
    }
}

/// Throughput metrics for [MeteredSender]
#[derive(Clone, Metrics)]
#[metrics(dynamic = true)]
struct MeteredSenderMetrics {
    /// Number of messages sent
    messages_sent: Counter,
    /// Number of failed message deliveries
    send_errors: Counter,
}

/// Throughput metrics for [MeteredReceiver]
#[derive(Clone, Metrics)]
#[metrics(dynamic = true)]
struct MeteredReceiverMetrics {
    /// Number of messages received
    messages_received: Counter,
}
