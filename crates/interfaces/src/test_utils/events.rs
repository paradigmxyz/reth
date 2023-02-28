use crate::events::{ChainEventSubscriptions, NewBlockNotification, NewBlockNotifications};
use async_trait::async_trait;
use reth_primitives::{Header, H256};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[derive(Default)]
/// A test ChainEventSubscriptions
pub struct TestChainEventSubscriptions {
    new_blocks_txs: Mutex<Vec<UnboundedSender<NewBlockNotification>>>,
}

impl TestChainEventSubscriptions {
    /// Instantiates an empty [`TestChainEventSubscriptions`] with no new blocks queued
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds new block to the queue that can be consumed with [`TestChainEventSubscriptions::subscribe_new_blocks`]
    pub fn add_new_block(&mut self, hash: H256, header: Header) {
        let header = Arc::new(header);
        self.new_blocks_txs
            .lock()
            .as_mut()
            .unwrap()
            .iter()
            .for_each(|tx| tx.send(NewBlockNotification { hash, header: header.clone() }).unwrap());
    }
}

impl ChainEventSubscriptions for TestChainEventSubscriptions {
    fn subscribe_new_blocks(&self) -> NewBlockNotifications {
        let (new_blocks_tx, new_blocks_rx) = unbounded_channel();
        self.new_blocks_txs.lock().as_mut().unwrap().push(new_blocks_tx);

        new_blocks_rx
    }
}
