use alloy_primitives::B256;
use reth_execution_types::BlockExecOutput;
use reth_primitives::{SealedBlockWithSenders, SealedHeader};
use reth_trie::updates::TrieUpdates;

/// An invalid block hook.
pub trait InvalidBlockHook<O>: Send + Sync {
    /// Invoked when an invalid block is encountered.
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &O,
        trie_updates: Option<(&TrieUpdates, B256)>,
    );
}

impl<F, O> InvalidBlockHook<O> for F
where
    F: Fn(&SealedHeader, &SealedBlockWithSenders, &O, Option<(&TrieUpdates, B256)>) + Send + Sync,
    O: BlockExecOutput,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &O,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        self(parent_header, block, output, trie_updates)
    }
}
