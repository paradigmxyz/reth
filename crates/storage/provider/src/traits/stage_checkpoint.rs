use reth_interfaces::Result;
use reth_primitives::{stage::StageId, BlockNumber, StageCheckpoint};

/// The trait for fetching stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointProvider: Send + Sync {
    /// Fetch the checkpoint for the given stage.
    fn get_stage_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>>;

    /// Fetch the checkpoint for the last stage ([reth_primitives::stage::StageId]).
    /// Should return `0` if no progress has been made yet.
    fn get_minimum_stage_checkpoint_block(&self) -> Result<BlockNumber>;
}
