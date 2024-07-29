use crate::BlockchainTreePendingStateProvider;
use reth_blockchain_tree_api::{BlockchainTreeEngine, BlockchainTreeViewer};
use reth_chain_state::CanonStateSubscriptions;

/// Helper trait to combine all the traits we need for the `BlockchainProvider`
///
/// This is a temporary solution
pub trait TreeViewer:
    BlockchainTreeViewer
    + BlockchainTreePendingStateProvider
    + CanonStateSubscriptions
    + BlockchainTreeEngine
{
}

impl<T> TreeViewer for T where
    T: BlockchainTreeViewer
        + BlockchainTreePendingStateProvider
        + CanonStateSubscriptions
        + BlockchainTreeEngine
{
}
