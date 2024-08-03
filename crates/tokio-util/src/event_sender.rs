use crate::EventStream;
use tokio::sync::broadcast::{self, Sender};
use tracing::trace;

const DEFAULT_SIZE_BROADCAST_CHANNEL: usize = 2000;

/// A bounded broadcast channel for a task.
#[derive(Debug, Clone)]
pub struct EventSender<T> {
    /// The sender part of the broadcast channel
    sender: Sender<T>,
}

impl<T> Default for EventSender<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new(DEFAULT_SIZE_BROADCAST_CHANNEL)
    }
}

impl<T: Clone + Send + Sync + 'static> EventSender<T> {
    /// Creates a new `EventSender`.
    pub fn new(events_channel_size: usize) -> Self {
        let (sender, _) = broadcast::channel(events_channel_size);
        Self { sender }
    }

    /// Broadcasts an event to all listeners.
    pub fn notify(&self, event: T) {
        if self.sender.send(event).is_err() {
            trace!("no receivers for broadcast events");
        }
    }

    /// Creates a new event stream with a subscriber to the sender as the
    /// receiver.
    pub fn new_listener(&self) -> EventStream<T> {
        EventStream::new(self.sender.subscribe())
    }
}
