use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader, B256};
use reth_provider::BlockExecutionOutput;
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
    );
}

impl<F> InvalidBlockHook for F
where
    F: Fn(
            &SealedBlockWithSenders,
            &SealedHeader,
            &BlockExecutionOutput<Receipt>,
            Option<(&TrieUpdates, B256)>,
        ) + Send
        + Sync,
{
    fn on_invalid_block(
        &self,
        block: &SealedBlockWithSenders,
        header: &SealedHeader,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
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
        _block: &SealedBlockWithSenders,
        _header: &SealedHeader,
        _output: &BlockExecutionOutput<Receipt>,
        _trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
    }
}

/// Multiple [`InvalidBlockHook`]s that are executed in order.
pub struct InvalidBlockHooks(pub Vec<Box<dyn InvalidBlockHook>>);

impl std::fmt::Debug for InvalidBlockHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvalidBlockHooks").field("len", &self.0.len()).finish()
    }
}

impl InvalidBlockHook for InvalidBlockHooks {
    fn on_invalid_block(
        &self,
        block: &SealedBlockWithSenders,
        header: &SealedHeader,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        for hook in &self.0 {
            hook.on_invalid_block(block, header, output, trie_updates);
        }
    }
}
