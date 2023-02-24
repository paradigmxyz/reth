use crate::events::{ChainEventSubscriptions, NewBlockNotification, NewBlockNotifications};
use reth_primitives::{Header, H256};
use std::{cell::RefCell, sync::Arc};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    Mutex,
};

#[derive(Default)]
pub struct TestChainEventSubscriptions {
    new_blocks_txs: RefCell<Vec<UnboundedSender<NewBlockNotification>>>,
}

impl TestChainEventSubscriptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_new_block(&mut self, hash: H256, header: Header) {
        let header = Arc::new(header.clone());
        self.new_blocks_txs
            .borrow()
            .iter()
            .for_each(|tx| tx.send(NewBlockNotification { hash, header: header.clone() }).unwrap());
    }
}

impl ChainEventSubscriptions for TestChainEventSubscriptions {
    fn subscribe_new_blocks(&self) -> NewBlockNotifications {
        let (new_blocks_tx, new_blocks_rx) = unbounded_channel();
        self.new_blocks_txs.borrow_mut().push(new_blocks_tx);

        new_blocks_rx
    }
}
