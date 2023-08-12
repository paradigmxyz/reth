use reth_interfaces::Result;
use reth_primitives::{PruneCheckpoint, PrunePart};

/// The trait for fetching prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointReader: Send + Sync {
    /// Fetch the checkpoint for the given prune part.
    fn get_prune_checkpoint(&self, part: PrunePart) -> Result<Option<PruneCheckpoint>>;
}

/// The trait for updating prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointWriter: Send + Sync {
    /// Save prune checkpoint.
    fn save_prune_checkpoint(&self, part: PrunePart, checkpoint: PruneCheckpoint) -> Result<()>;
}
