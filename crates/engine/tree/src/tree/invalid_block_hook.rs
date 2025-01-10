use alloy_primitives::B256;
use reth_engine_primitives::InvalidBlockHook;
use reth_primitives::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_provider::BlockExecutionOutput;
use reth_trie::updates::TrieUpdates;

/// A no-op [`InvalidBlockHook`] that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopInvalidBlockHook;

impl<N: NodePrimitives> InvalidBlockHook<N> for NoopInvalidBlockHook {
    fn on_invalid_block(
        &self,
        _parent_header: &SealedHeader<N::BlockHeader>,
        _block: &RecoveredBlock<N::Block>,
        _output: &BlockExecutionOutput<N::Receipt>,
        _trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
    }
}

/// Multiple [`InvalidBlockHook`]s that are executed in order.
pub struct InvalidBlockHooks<N: NodePrimitives>(pub Vec<Box<dyn InvalidBlockHook<N>>>);

impl<N: NodePrimitives> std::fmt::Debug for InvalidBlockHooks<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvalidBlockHooks").field("len", &self.0.len()).finish()
    }
}

impl<N: NodePrimitives> InvalidBlockHook<N> for InvalidBlockHooks<N> {
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        for hook in &self.0 {
            hook.on_invalid_block(parent_header, block, output, trie_updates);
        }
    }
}
