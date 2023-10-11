//! Blockchain tree state.

use crate::BlockIndices;
use parking_lot::RwLock;

/// Container to hold the state of the blockchain tree.
#[derive(Debug, Default)]
pub(crate) struct TreeState {
    /// Indices to block and their connection to the canonical chain.
    ///
    /// This gets modified by the tree itself and is read from engine API/RPC to access the pending block for example.
    block_indices: RwLock<BlockIndices>,
}

impl TreeState {

}