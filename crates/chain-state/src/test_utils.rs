use crate::{
    in_memory::ExecutedBlock, CanonStateNotification, CanonStateNotifications,
    CanonStateSubscriptions,
};
use rand::Rng;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::{
    Address, Block, BlockNumber, Receipts, Requests, SealedBlockWithSenders, TransactionSigned,
};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use revm::db::BundleState;
use std::{
    ops::Range,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast::{self, Sender};

fn get_executed_block(block_number: BlockNumber, receipts: Receipts) -> ExecutedBlock {
    let mut block = Block::default();
    let mut header = block.header.clone();
    header.number = block_number;
    block.header = header;

    let sender = Address::random();
    let tx = TransactionSigned::default();
    block.body.push(tx);
    let sealed = block.seal_slow();
    let sealed_with_senders = SealedBlockWithSenders::new(sealed.clone(), vec![sender]).unwrap();

    ExecutedBlock::new(
        Arc::new(sealed),
        Arc::new(sealed_with_senders.senders),
        Arc::new(ExecutionOutcome::new(
            BundleState::default(),
            receipts,
            block_number,
            vec![Requests::default()],
        )),
        Arc::new(HashedPostState::default()),
        Arc::new(TrieUpdates::default()),
    )
}

/// Generates an `ExecutedBlock` that includes the given `Receipts`.
pub fn get_executed_block_with_receipts(receipts: Receipts) -> ExecutedBlock {
    let number = rand::thread_rng().gen::<u64>();

    get_executed_block(number, receipts)
}

/// Generates an `ExecutedBlock` with the given `BlockNumber`.
pub fn get_executed_block_with_number(block_number: BlockNumber) -> ExecutedBlock {
    get_executed_block(block_number, Receipts { receipt_vec: vec![vec![]] })
}

/// Generates a range of executed blocks with ascending block numbers.
pub fn get_executed_blocks(range: Range<u64>) -> impl Iterator<Item = ExecutedBlock> {
    range.map(get_executed_block_with_number)
}

/// A test `ChainEventSubscriptions`
#[derive(Clone, Debug, Default)]
pub struct TestCanonStateSubscriptions {
    canon_notif_tx: Arc<Mutex<Vec<Sender<CanonStateNotification>>>>,
}

impl TestCanonStateSubscriptions {
    /// Adds new block commit to the queue that can be consumed with
    /// [`TestCanonStateSubscriptions::subscribe_to_canonical_state`]
    pub fn add_next_commit(&self, new: Arc<Chain>) {
        let event = CanonStateNotification::Commit { new };
        self.canon_notif_tx.lock().as_mut().unwrap().retain(|tx| tx.send(event.clone()).is_ok())
    }

    /// Adds reorg to the queue that can be consumed with
    /// [`TestCanonStateSubscriptions::subscribe_to_canonical_state`]
    pub fn add_next_reorg(&self, old: Arc<Chain>, new: Arc<Chain>) {
        let event = CanonStateNotification::Reorg { old, new };
        self.canon_notif_tx.lock().as_mut().unwrap().retain(|tx| tx.send(event.clone()).is_ok())
    }
}

impl CanonStateSubscriptions for TestCanonStateSubscriptions {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        let (canon_notif_tx, canon_notif_rx) = broadcast::channel(100);
        self.canon_notif_tx.lock().as_mut().unwrap().push(canon_notif_tx);

        canon_notif_rx
    }
}
