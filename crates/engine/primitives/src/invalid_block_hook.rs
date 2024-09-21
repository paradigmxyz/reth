use alloy_primitives::B256;
use reth_execution_types::BlockExecutionOutput;
use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader};
use reth_trie::updates::TrieUpdates;

/// An invalid block hook.
pub trait InvalidBlockHook: Send + Sync {
    /// Invoked when an invalid block is encountered.
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    );
}

impl<F> InvalidBlockHook for F
where
    F: Fn(
            &SealedHeader,
            &SealedBlockWithSenders,
            &BlockExecutionOutput<Receipt>,
            Option<(&TrieUpdates, B256)>,
        ) + Send
        + Sync,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        self(parent_header, block, output, trie_updates)
    }
}
