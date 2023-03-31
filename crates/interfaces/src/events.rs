use reth_primitives::SealedHeader;
use std::sync::Arc;
use tokio::sync::broadcast::{Receiver, Sender};

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
