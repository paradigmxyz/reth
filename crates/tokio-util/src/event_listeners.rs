use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::broadcast::{self, error::SendError, Sender};
use tokio_stream::wrappers::BroadcastStream;
use tracing::{error, warn};

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

/// Notifies events to the listeners subscribed to the wrapped broadcast sender.
#[derive(Debug)]
pub struct EventNotifier<T> {
    sender: Option<Sender<T>>,
}

impl<T: Send + Sync + 'static> Default for EventNotifier<T> {
    fn default() -> Self {
        Self { sender: None }
    }
}

impl<T: Send + Sync + 'static> EventNotifier<T> {
    /// Broadcast sender setter.
    pub fn set_sender(&mut self, sender: Sender<T>) {
        self.sender = Some(sender);
    }

    /// Sends an event to all listeners. Returns the number of subscribers the event was sent to.
    pub fn notify(&self, event: T) {
        match self.sender.as_ref().map(|sender| sender.send(event)).unwrap_or(Ok(0)) {
            Ok(listener_count) => {
                if listener_count == 0 {
                    warn!("notification of network event with 0 listeners");
                }
            }
            Err(e) => error!("channel closed: {e}"),
        };
    }
}
