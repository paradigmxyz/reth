use crate::{BlockchainTreePendingStateProvider, CanonStateSubscriptions};

use reth_interfaces::blockchain_tree::{BlockchainTreeEngine, BlockchainTreeViewer};

/// Helper trait to combine all the traits we need for the BlockchainProvider
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
