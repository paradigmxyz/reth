use reth_interfaces::RethResult;
use reth_primitives::{PruneCheckpoint, PruneSegment};

/// The trait for fetching prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointReader: Send + Sync {
    /// Fetch the checkpoint for the given prune segment.
    fn get_prune_checkpoint(&self, segment: PruneSegment) -> RethResult<Option<PruneCheckpoint>>;
}

/// The trait for updating prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointWriter: Send + Sync {
    /// Save prune checkpoint.
    fn save_prune_checkpoint(
        &self,
        segment: PruneSegment,
        checkpoint: PruneCheckpoint,
    ) -> RethResult<()>;
}
