use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{stage::StageId, BlockNumber, StageCheckpoint};

/// Client trait for fetching checkpoint related data.
#[auto_impl(&, Arc)]
pub trait PipelineProvider: Send + Sync {
    /// Get the last committed progress of a given stage.
    fn get_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>>;

    /// Get minimum block number processed by the pipeline.
    fn get_minimum_checkpoint_block(&self) -> Result<BlockNumber>;
}
