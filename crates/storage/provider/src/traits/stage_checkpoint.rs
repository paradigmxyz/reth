use reth_interfaces::Result;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber,
};

/// The trait for fetching stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointReader: Send + Sync {
    /// Fetch the checkpoint for the given stage.
    fn get_stage_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>>;

    /// Get stage checkpoint progress.
    fn get_stage_checkpoint_progress(&self, id: StageId) -> Result<Option<Vec<u8>>>;
}

/// The trait for updating stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointWriter: Send + Sync {
    /// Save stage checkpoint.
    fn save_stage_checkpoint(&self, id: StageId, checkpoint: StageCheckpoint) -> Result<()>;

    /// Save stage checkpoint progress.
    fn save_stage_checkpoint_progress(&self, id: StageId, checkpoint: Vec<u8>) -> Result<()>;

    /// Update all pipeline sync stage progress.
    fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
        drop_stage_checkpoint: bool,
    ) -> Result<()>;
}
