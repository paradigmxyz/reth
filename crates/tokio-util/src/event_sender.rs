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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        task,
        time::{timeout, Duration},
    };
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn test_event_broadcast_to_listener() {
        let sender = EventSender::default();

        // Create a listener for the events
        let mut listener = sender.new_listener();

        // Broadcast an event
        sender.notify("event1");

        // Check if the listener receives the event
        let received_event = listener.next().await;
        assert_eq!(received_event, Some("event1"));
    }

    #[tokio::test]
    async fn test_event_no_listener() {
        let sender = EventSender::default();

        // Broadcast an event with no listeners
        sender.notify("event2");

        // Ensure it doesn't panic or fail when no listeners are present
        // (this test passes if it runs without errors).
    }

    #[tokio::test]
    async fn test_multiple_listeners_receive_event() {
        let sender = EventSender::default();

        // Create two listeners
        let mut listener1 = sender.new_listener();
        let mut listener2 = sender.new_listener();

        // Broadcast an event
        sender.notify("event3");

        // Both listeners should receive the same event
        let event1 = listener1.next().await;
        let event2 = listener2.next().await;

        assert_eq!(event1, Some("event3"));
        assert_eq!(event2, Some("event3"));
    }

    #[tokio::test]
    async fn test_bounded_channel_size() {
        // Create a channel with size 2
        let sender = EventSender::new(2);

        // Create a listener
        let mut listener = sender.new_listener();

        // Broadcast 3 events, which exceeds the channel size
        sender.notify("event4");
        sender.notify("event5");
        sender.notify("event6");

        // Only the last two should be received due to the size limit
        let received_event1 = listener.next().await;
        let received_event2 = listener.next().await;

        assert_eq!(received_event1, Some("event5"));
        assert_eq!(received_event2, Some("event6"));
    }

    #[tokio::test]
    async fn test_event_listener_timeout() {
        let sender = EventSender::default();
        let mut listener = sender.new_listener();

        // Broadcast an event asynchronously
        task::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            sender.notify("delayed_event");
        });

        // Use a timeout to ensure that the event is received within a certain time
        let result = timeout(Duration::from_millis(100), listener.next()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some("delayed_event"));
    }
}
