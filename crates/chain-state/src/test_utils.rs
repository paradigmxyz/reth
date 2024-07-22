use crate::in_memory::ExecutedBlock;
use rand::Rng;
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    Address, Block, BlockNumber, Receipts, Requests, SealedBlockWithSenders, TransactionSigned,
};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use revm::db::BundleState;
use std::{ops::Range, sync::Arc};

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
