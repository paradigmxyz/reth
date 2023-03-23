use crate::events::{ChainEventSubscriptions, NewBlockNotification, NewBlockNotifications};
use async_trait::async_trait;
use reth_primitives::{Header, H256};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// A test ChainEventSubscriptions
#[derive(Clone, Default)]
pub struct TestChainEventSubscriptions {
    new_blocks_txs: Arc<Mutex<Vec<UnboundedSender<NewBlockNotification>>>>,
}

impl TestChainEventSubscriptions {
    /// Adds new block to the queue that can be consumed with
    /// [`TestChainEventSubscriptions::subscribe_new_blocks`]
    pub fn add_new_block(&mut self, hash: H256, header: Header) {
        let header = Arc::new(header);
        self.new_blocks_txs
            .lock()
            .as_mut()
            .unwrap()
            .retain(|tx| tx.send(NewBlockNotification { hash, header: header.clone() }).is_ok())
    }
}

impl ChainEventSubscriptions for TestChainEventSubscriptions {
    fn subscribe_new_blocks(&self) -> NewBlockNotifications {
        let (new_blocks_tx, new_blocks_rx) = unbounded_channel();
        self.new_blocks_txs.lock().as_mut().unwrap().push(new_blocks_tx);

        new_blocks_rx
    }
}
