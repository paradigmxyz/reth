use crate::tree::TreeState;
use parking_lot::RwLock;
use reth_primitives::B256;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

// TODO: design questions:
// * to support "unwinds" or not? ie, what do we do when we get a request to persist a chain that is
// overlapping with the one on disk
/// Writes parts of the tree to the database
pub struct Persistence<Writer> {
    // TODO: use the right type for the writer
    /// The db / static file provider to use
    provider: Writer,
    /// The tree state
    tree_state: Arc<RwLock<TreeState>>,
    // TODO: handles for pushing requests
    /// Incoming requests to persist stuff
    incoming: VecDeque<PersistenceAction>,
}

impl<Writer> Persistence<Writer> {
    // TODO: initialization
    /// Writes the cloned tree state to the database
    fn write(&self) {
        todo!("need tree state to write")
    }
}

impl<Writer> Persistence<Writer>
where
    Writer: Unpin,
{
    /// Internal method to poll the persistence task
    #[tracing::instrument(level = "debug", name = "Persistence::poll", skip(self, cx))]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        // get most recent action
        // drain actions to get the largest range that should be persisted?
        let mut range_end = None;
        // TODO: drain incoming for range end

        if let Some(block_number) = range_end {
            // clone tree state
            // write cloned diff to disk
            // produce updates for in memory tree
            let mut tree = this.tree_state.write();
            // todo: ensure there was no change to the blocks before lowest_block_number
            tree.remove_before(block_number);
            // update tree
        }

        Poll::Pending
    }
}

/// A signal to the persistence task that part of the tree state can be persisted.
pub enum PersistenceAction {
    /// The tree state from before this block can be persisted
    SaveBlocks(u64),

    // TODO: maybe not necessary
    /// The tree state from before this block hash has been finalized and can be persisted
    SaveFinalizedBlock(B256),
}
