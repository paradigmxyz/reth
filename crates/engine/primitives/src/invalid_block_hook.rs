use alloc::{boxed::Box, fmt, vec::Vec};
use alloy_primitives::B256;
use reth_execution_types::BlockExecutionOutput;
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_trie_common::updates::TrieUpdates;

/// An invalid block hook.
pub trait InvalidBlockHook<N: NodePrimitives>: Send + Sync {
    /// Invoked when an invalid block is encountered.
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    );
}

impl<F, N> InvalidBlockHook<N> for F
where
    N: NodePrimitives,
    F: Fn(
            &SealedHeader<N::BlockHeader>,
            &RecoveredBlock<N::Block>,
            &BlockExecutionOutput<N::Receipt>,
            Option<(&TrieUpdates, B256)>,
        ) + Send
        + Sync,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        self(parent_header, block, output, trie_updates)
    }
}

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

impl<N: NodePrimitives> fmt::Debug for InvalidBlockHooks<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
