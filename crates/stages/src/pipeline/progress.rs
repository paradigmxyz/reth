use super::ctrl::ControlFlow;
use crate::util::opt;
use reth_interfaces::sync::SyncState;
use reth_primitives::BlockNumber;

#[derive(Debug, Default)]
pub(crate) struct PipelineProgress {
    /// The progress of the current stage
    pub(crate) progress: Option<BlockNumber>,
    /// The maximum progress achieved by any stage during the execution of the pipeline.
    pub(crate) maximum_progress: Option<BlockNumber>,
    /// The minimum progress achieved by any stage during the execution of the pipeline.
    pub(crate) minimum_progress: Option<BlockNumber>,
}

impl PipelineProgress {
    pub(crate) fn update(&mut self, progress: BlockNumber) {
        self.progress = Some(progress);
        self.minimum_progress = opt::min(self.minimum_progress, progress);
        self.maximum_progress = opt::max(self.maximum_progress, progress);
    }

    /// Create a sync state from pipeline progress.
    pub(crate) fn current_sync_state(&self, downloading: bool) -> SyncState {
        match self.progress {
            Some(progress) if downloading => SyncState::Downloading { target_block: progress },
            Some(progress) => SyncState::Executing { target_block: progress },
            None => SyncState::Idle,
        }
    }

    /// Get next control flow step
    pub(crate) fn next_ctrl(&self) -> ControlFlow {
        match self.progress {
            Some(progress) => ControlFlow::Continue { progress },
            None => ControlFlow::NoProgress { stage_progress: None },
        }
    }
}
