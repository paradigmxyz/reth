use crate::{util::opt, ControlFlow};
use reth_primitives::BlockNumber;

#[derive(Debug, Default)]
pub struct PipelineProgress {
    /// Block number reached by the stage.
    pub block_number: Option<BlockNumber>,
    /// The maximum block number achieved by any stage during the execution of the pipeline.
    pub maximum_block_number: Option<BlockNumber>,
    /// The minimum block number achieved by any stage during the execution of the pipeline.
    pub minimum_block_number: Option<BlockNumber>,
}

impl PipelineProgress {
    pub(crate) fn update(&mut self, block_number: BlockNumber) {
        self.block_number = Some(block_number);
        self.minimum_block_number = opt::min(self.minimum_block_number, block_number);
        self.maximum_block_number = opt::max(self.maximum_block_number, block_number);
    }

    /// Get next control flow step
    pub(crate) fn next_ctrl(&self) -> ControlFlow {
        match self.block_number {
            Some(block_number) => ControlFlow::Continue { block_number },
            None => ControlFlow::NoProgress { block_number: None },
        }
    }
}
