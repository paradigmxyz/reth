use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader, B256};
use reth_provider::BlockExecutionOutput;
use reth_trie::updates::TrieUpdates;

/// A bad block hook.
pub trait InvalidBlockHook: Send + Sync {
    /// Invoked when a bad block is encountered.
    fn on_invalid_block(
        &self,
        block: SealedBlockWithSenders,
        header: SealedHeader,
        output: BlockExecutionOutput<Receipt>,
        trie_updates: Option<(TrieUpdates, B256)>,
    );
}

impl<F> InvalidBlockHook for F
where
    F: Fn(
            SealedBlockWithSenders,
            SealedHeader,
            BlockExecutionOutput<Receipt>,
            Option<(TrieUpdates, B256)>,
        ) + Send
        + Sync,
{
    fn on_invalid_block(
        &self,
        block: SealedBlockWithSenders,
        header: SealedHeader,
        output: BlockExecutionOutput<Receipt>,
        trie_updates: Option<(TrieUpdates, B256)>,
    ) {
        self(block, header, output, trie_updates)
    }
}

/// A no-op [`InvalidBlockHook`] that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopInvalidBlockHook;

impl InvalidBlockHook for NoopInvalidBlockHook {
    fn on_invalid_block(
        &self,
        _block: SealedBlockWithSenders,
        _header: SealedHeader,
        _output: BlockExecutionOutput<Receipt>,
        _trie_updates: Option<(TrieUpdates, B256)>,
    ) {
    }
}
