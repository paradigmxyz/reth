use alloy_primitives::B256;
use reth_engine_primitives::InvalidBlockHook;
use reth_primitives::{Receipt, SealedBlockWithSenders, SealedHeader};
use reth_provider::BlockExecutionOutput;
use reth_trie::updates::TrieUpdates;

/// A no-op [`InvalidBlockHook`] that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopInvalidBlockHook;

impl InvalidBlockHook for NoopInvalidBlockHook {
    fn on_invalid_block(
        &self,
        _parent_header: &SealedHeader,
        _block: &SealedBlockWithSenders,
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
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        for hook in &self.0 {
            hook.on_invalid_block(parent_header, block, output, trie_updates);
        }
    }
}
