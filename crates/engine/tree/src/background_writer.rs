use std::sync::Arc;
use parking_lot::RwLock;
use reth_primitives::B256;
use crate::tree::TreeState;

// TODO: design questions:
// * to support "unwinds" or not? ie, what do we do when we get a request to persist a chain that is
// overlapping with the one on disk
/// Writes parts of the tree to the database
pub struct Persistence<Writer> {
    /// The db / static file provider to use
    provider: Writer,
    /// The locked tree state
    tree_state: Arc<RwLock<TreeState>>,
}

impl<Writer> Persistence<Writer> {
    /// Writes the cloned tree state to the database
    pub fn write(&self) {
        todo!("need tree state to write")
    }
}

/// A signal to the persistence task that part of the tree state can be persisted.
pub enum PersistenceAction {
    // TODO: support reorgs? ie, save these blocks, and re-add the overwritten blocks to the state?
    /// The tree state from before this block can be persisted
    SaveBlocks(u64),

    // TODO: maybe not necessary
    /// The tree state from before this block hash has been finalized and can be persisted
    SaveFinalizedBlock(B256),
}
