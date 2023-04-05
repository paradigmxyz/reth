use parking_lot::Mutex;
use reth_primitives::SealedHeader;
use std::sync::Arc;
use tokio::sync::broadcast::{error::SendError, Receiver, Sender};

/// New block notification that is Arc around [SealedHeader].
pub type NewBlockNotification = Arc<SealedHeader>;

/// Type alias for a receiver that receives [NewBlockNotification]
pub type NewBlockNotifications = Receiver<NewBlockNotification>;

/// Type alias for a sender that sends [NewBlockNotification]
pub type NewBlockNotificationsSender = Sender<NewBlockNotification>;

/// A type that allows to register chain related event subscriptions.
pub trait ChainEventSubscriptions: Send + Sync {
    /// Get notified when a new block was imported.
    fn subscribe_new_blocks(&self) -> NewBlockNotifications;
}

/// A shareable Sender that allows to send [NewBlockNotification] to all receivers.
#[derive(Debug, Clone)]
pub struct NewBlockNotificationSink {
    inner: Arc<Mutex<Sender<NewBlockNotification>>>,
}

// === impl NewBlockNotificationSink ===

impl NewBlockNotificationSink {
    /// Creates a new NewBlockNotificationSink with the given capacity.
    // // size of the broadcast is double of max reorg depth because at max reorg depth we can have
    //         // send at least N block at the time.
    pub fn new(capacity: usize) -> Self {
        let inner = tokio::sync::broadcast::channel(capacity);
        Self { inner: Arc::new(Mutex::new(inner.0)) }
    }

    /// Attempts to send a value to all active Receiver handles, returning it back if it could not
    /// be sent.
    pub fn send(
        &self,
        header: NewBlockNotification,
    ) -> Result<usize, SendError<NewBlockNotification>> {
        let sender = self.inner.lock();
        sender.send(header)
    }

    /// Creates a new Receiver handle that will receive notifications sent after this call to
    /// subscribe.
    pub fn subscribe(&self) -> Receiver<NewBlockNotification> {
        let sender = self.inner.lock();
        sender.subscribe()
    }
}
