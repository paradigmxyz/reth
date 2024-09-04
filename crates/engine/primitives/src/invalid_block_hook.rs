use reth_execution_types::BlockExecutionOutput;
use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader, B256};
use reth_trie::updates::TrieUpdates;

/// A bad block hook.
pub trait InvalidBlockHook: Send + Sync {
    /// Invoked when a bad block is encountered.
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()>;
}

impl<F> InvalidBlockHook for F
where
    F: Fn(
            &SealedHeader,
            &SealedBlockWithSenders,
            &BlockExecutionOutput<Receipt>,
            Option<(&TrieUpdates, B256)>,
        ) -> eyre::Result<()>
        + Send
        + Sync,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()> {
        self(parent_header, block, output, trie_updates)
    }
}
