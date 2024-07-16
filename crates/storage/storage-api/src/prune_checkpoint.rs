use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_storage_errors::provider::ProviderResult;

/// The trait for fetching prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointReader: Send + Sync {
    /// Fetch the prune checkpoint for the given segment.
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>>;

    /// Fetch all the prune checkpoints.
    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>>;
}

/// The trait for updating prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointWriter: Send + Sync {
    /// Save prune checkpoint.
    fn save_prune_checkpoint(
        &self,
        segment: PruneSegment,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()>;
}
