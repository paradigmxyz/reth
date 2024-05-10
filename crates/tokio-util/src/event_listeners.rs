use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::broadcast::{self, error::SendError, Sender};
use tokio_stream::wrappers::BroadcastStream;

/// A bounded broadcast channel for a task.
#[derive(Debug)]
pub struct EventListeners<T> {
    /// The sender part of the broadcast channel
    sender: broadcast::Sender<T>,
    /// The number of subscribers, needed because the broadcast sender doesn't track this
    subscriber_count: AtomicUsize,
}

impl<T: Clone> Clone for EventListeners<T> {
    fn clone(&self) -> Self {
        EventListeners {
            sender: self.sender.clone(),
            subscriber_count: AtomicUsize::new(self.subscriber_count.load(Ordering::SeqCst)),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> Default for EventListeners<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + Send + Sync + 'static> EventListeners<T> {
    /// Creates a new `EventListeners`.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(100);
        Self { sender, subscriber_count: 0.into() }
    }

    /// Sends an event to all listeners. Returns the number of subscribers the event was sent to.
    pub fn notify(&self, event: T) -> Result<usize, SendError<T>> {
        self.sender.send(event)
    }

    /// Sender cloner.
    pub fn clone_sender(&self) -> Sender<T> {
        self.sender.clone()
    }

    /// Adds a new event listener and returns the associated receiver.
    pub fn new_listener(&self) -> BroadcastStream<T> {
        self.subscriber_count.fetch_add(1, Ordering::Relaxed);
        BroadcastStream::new(self.sender.subscribe())
    }

    /// Returns the number of registered listeners.
    pub fn len(&self) -> usize {
        self.subscriber_count.load(Ordering::Relaxed)
    }

    /// Returns true if there are no registered listeners.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
