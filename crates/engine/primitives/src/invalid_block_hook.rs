use alloy_primitives::B256;
use reth_execution_types::BlockExecutionOutput;
use reth_primitives::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_trie::updates::TrieUpdates;

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
