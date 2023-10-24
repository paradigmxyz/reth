use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A collection of event listeners for a task.
#[derive(Clone, Debug)]
pub struct EventListeners<T> {
    /// All listeners for events
    listeners: Vec<mpsc::UnboundedSender<T>>,
}

impl<T> Default for EventListeners<T> {
    fn default() -> Self {
        Self { listeners: Vec::new() }
    }
}

impl<T: Clone> EventListeners<T> {
    /// Send an event to all listeners.
    ///
    /// Channels that were closed are removed.
    pub fn notify(&mut self, event: T) {
        self.listeners.retain(|listener| listener.send(event.clone()).is_ok())
    }

    /// Add a new event listener.
    pub fn new_listener(&mut self) -> UnboundedReceiverStream<T> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.listeners.push(sender);
        UnboundedReceiverStream::new(receiver)
    }

    /// Push new event listener.
    pub fn push_listener(&mut self, listener: mpsc::UnboundedSender<T>) {
        self.listeners.push(listener);
    }

    /// Returns the number of registered listeners.
    pub fn len(&self) -> usize {
        self.listeners.len()
    }

    /// Returns true if there are no registered listeners.
    pub fn is_empty(&self) -> bool {
        self.listeners.is_empty()
    }
    /// Retrieves a reference to a listener at the specified index.

    /// An Option containing a reference to the listener if it exists, or `None` otherwise.
    pub fn get(&self, index: usize) -> Option<&mpsc::UnboundedSender<T>> {
        self.listeners.get(index)
    }
    /// Checks if a specific listener is still available.
    /// Returns `true` if the listener is available and `false` otherwise.

    pub fn is_listener_available(&self, index: usize) -> bool {
        if let Some(listener) = self.listeners.get(index) {
            !listener.is_closed()
        } else {
            false
        }
    }
    /// Finds a listener based on a given predicate.
    /// Returns an `Option` containing a reference to the first listener satisfying the predicate,
    /// or `None` if no listener matches.
    pub fn find<P>(&self, predicate: P) -> Option<&mpsc::UnboundedSender<T>>
    where
        P: Fn(&mpsc::UnboundedSender<T>) -> bool,
    {
        self.listeners.iter().find(|&listener| predicate(listener))
    }
    /// Returns an iterator over the listeners.

    pub fn iter(&self) -> impl Iterator<Item = &mpsc::UnboundedSender<T>> {
        self.listeners.iter()
    }

    /// Retain only the listeners that satisfy the specified predicate.

    pub fn filter<P>(&mut self, predicate: P)
    where
        P: Fn(&mpsc::UnboundedSender<T>) -> bool,
    {
        self.listeners.retain(predicate);
    }

    /// Returns the total capacity of the internal listener vector.

    pub fn capacity(&self) -> usize {
        self.listeners.capacity()
    }
}
