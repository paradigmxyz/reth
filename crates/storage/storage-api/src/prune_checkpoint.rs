use alloc::vec::Vec;
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_storage_errors::provider::ProviderResult;

/// The trait for fetching prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointReader: Send {
    /// Fetch the prune checkpoint for the given segment.
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>>;

    /// Fetch all the prune checkpoints.
    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>>;

    /// Fetch checkpoints for reporting data availability, accounting for data still retained in
    /// other storage backends. This does not modify the stored checkpoints.
    ///
    /// The default returns the stored checkpoints. Implementations may lower their block and
    /// transaction numbers when continuous coverage can be verified, preserving the prune mode.
    /// Use [`Self::get_prune_checkpoints`] for pruning progress instead.
    fn get_prune_checkpoints_for_availability(
        &self,
    ) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        self.get_prune_checkpoints()
    }
}

/// The trait for updating prune checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait PruneCheckpointWriter {
    /// Save prune checkpoint.
    fn save_prune_checkpoint(
        &self,
        segment: PruneSegment,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()>;
}
