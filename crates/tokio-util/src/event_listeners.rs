use tokio::sync::broadcast::{self, Sender};
use tokio_stream::wrappers::BroadcastStream;
use tracing::error;

const DEFAULT_SIZE_BROADCAST_CHANNEL: usize = 1000;

/// A bounded broadcast channel for a task.
#[derive(Debug, Clone)]
pub struct EventListeners<T> {
    /// The sender part of the broadcast channel
    sender: Sender<T>,
}

impl<T: Clone + Send + Sync + 'static> Default for EventListeners<T> {
    fn default() -> Self {
        Self::new(DEFAULT_SIZE_BROADCAST_CHANNEL)
    }
}

impl<T: Clone + Send + Sync + 'static> EventListeners<T> {
    /// Creates a new `EventListeners`.
    pub fn new(broadcast_channel_size: usize) -> Self {
        let (sender, _) = broadcast::channel(broadcast_channel_size);
        Self { sender }
    }

    /// Broadcasts an event to all listeners.
    pub fn notify(&self, event: T) {
        if self.sender.send(event).is_err() {
            error!("channel closed");
        }
    }

    /// Adds a new event listener and returns the associated receiver.
    pub fn new_listener(&self) -> BroadcastStream<T> {
        BroadcastStream::new(self.sender.subscribe())
    }
}
