//! crate

use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader, B256};
use reth_provider::BlockExecutionOutput;
use reth_trie::updates::TrieUpdates;

pub fn witness_invalid_block_hook(
    block: SealedBlockWithSenders,
    header: SealedHeader,
    output: BlockExecutionOutput<Receipt>,
    trie_updates: Option<(TrieUpdates, B256)>,
) {
    // do nothing
}
