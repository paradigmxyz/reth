use reth_interfaces::Result;
use reth_primitives::{stage::StageId, BlockNumber, StageCheckpoint};

/// The trait for fetching stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointProvider: Send + Sync {
    /// Fetch the checkpoint for the given stage.
    fn get_stage_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>>;

    /// Fetch the checkpoint for [reth_primitives::stage::StageId::Finish] stage.
    /// Should return `0` if no progress has been made yet.
    ///
    /// NOTE: This checkpoint is always `<=` to the checkpoint of any other stage. It essentially
    /// represents the fully synced block.
    fn get_finish_stage_checkpoint_block_number(&self) -> Result<BlockNumber>;
}
