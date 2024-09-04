use reth_execution_types::BlockExecutionOutput;
use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader, B256};
use reth_trie::updates::TrieUpdates;

/// A bad block hook.
pub trait InvalidBlockHook: Send + Sync {
    /// Invoked when a bad block is encountered.
    fn on_invalid_block(
        &self,
        block: &SealedBlockWithSenders,
        header: &SealedHeader,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()>;
}

impl<F> InvalidBlockHook for F
where
    F: Fn(
            &SealedBlockWithSenders,
            &SealedHeader,
            &BlockExecutionOutput<Receipt>,
            Option<(&TrieUpdates, B256)>,
        ) -> eyre::Result<()>
        + Send
        + Sync,
{
    fn on_invalid_block(
        &self,
        block: &SealedBlockWithSenders,
        header: &SealedHeader,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()> {
        self(block, header, output, trie_updates)
    }
}
