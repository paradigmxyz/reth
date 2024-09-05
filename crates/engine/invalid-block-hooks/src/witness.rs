use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader, B256};
use reth_provider::BlockExecutionOutput;
use reth_trie::updates::TrieUpdates;

/// Generates a witness for the given block and saves it to a file.
pub fn witness(
    _block: &SealedBlockWithSenders,
    _header: &SealedHeader,
    _output: &BlockExecutionOutput<Receipt>,
    _trie_updates: Option<(&TrieUpdates, B256)>,
) {
    unimplemented!("witness generation is not supported")
}
