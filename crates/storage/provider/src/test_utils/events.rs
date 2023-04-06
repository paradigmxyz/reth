use std::sync::{Arc, Mutex};
use tokio::sync::broadcast::{self, Sender};

use crate::{CanonStateNotification, CanonStateNotifications, CanonStateSubscriptions, SubChain};

/// A test ChainEventSubscriptions
#[derive(Clone, Default)]
pub struct TestCanonStateSubscriptions {
    canon_notif_tx: Arc<Mutex<Vec<Sender<CanonStateNotification>>>>,
}

impl TestCanonStateSubscriptions {
    /// Adds new block commit to the queue that can be consumed with
    /// [`TestChainEventSubscriptions::subscribe_new_blocks`]
    pub fn add_next_commit(&mut self, new: Arc<dyn SubChain>) {
        let event = CanonStateNotification::Commit { new: new.clone() };
        self.canon_notif_tx.lock().as_mut().unwrap().retain(|tx| tx.send(event.clone()).is_ok())
    }

    /// Adds reorg to the queue that can be consumed with
    /// [`TestChainEventSubscriptions::subscribe_new_blocks`]
    pub fn add_next_reorg(&mut self, old: Arc<dyn SubChain>, new: Arc<dyn SubChain>) {
        let event = CanonStateNotification::Reorg { old, new };
        self.canon_notif_tx.lock().as_mut().unwrap().retain(|tx| tx.send(event.clone()).is_ok())
    }
}

impl CanonStateSubscriptions for TestCanonStateSubscriptions {
    fn subscribe_canon_state(&self) -> CanonStateNotifications {
        let (canon_notif_tx, canon_notif_rx) = broadcast::channel(100);
        self.canon_notif_tx.lock().as_mut().unwrap().push(canon_notif_tx);

        canon_notif_rx
    }
}
