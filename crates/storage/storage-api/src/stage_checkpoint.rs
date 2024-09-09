use alloy_primitives::BlockNumber;
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_errors::provider::ProviderResult;

/// The trait for fetching stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointReader: Send + Sync {
    /// Fetch the checkpoint for the given stage.
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>>;

    /// Get stage checkpoint progress.
    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>>;

    /// Reads all stage checkpoints and returns a list with the name of the stage and the checkpoint
    /// data.
    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>>;
}

/// The trait for updating stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointWriter: Send + Sync {
    /// Save stage checkpoint.
    fn save_stage_checkpoint(&self, id: StageId, checkpoint: StageCheckpoint)
        -> ProviderResult<()>;

    /// Save stage checkpoint progress.
    fn save_stage_checkpoint_progress(
        &self,
        id: StageId,
        checkpoint: Vec<u8>,
    ) -> ProviderResult<()>;

    /// Update all pipeline sync stage progress.
    fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
        drop_stage_checkpoint: bool,
    ) -> ProviderResult<()>;
}
