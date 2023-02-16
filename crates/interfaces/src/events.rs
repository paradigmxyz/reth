use reth_primitives::{Header, H256};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;

/// Type alias for a receiver that receives [NewBlockNotification]
pub type NewBlockNotifications = UnboundedReceiver<NewBlockNotification>;

/// A type that allows to register chain related event subscriptions.
pub trait ChainEventSubscriptions {
    /// Get notified when a new block was imported.
    fn subscribe_new_blocks(&self) -> NewBlockNotifications;
}

/// A notification that's emitted when a new block was imported.
#[derive(Clone, Debug)]
pub struct NewBlockNotification {
    /// Hash of the block that was imported
    pub hash: H256,
    /// The block header of the new block
    pub header: Arc<Header>,
}
