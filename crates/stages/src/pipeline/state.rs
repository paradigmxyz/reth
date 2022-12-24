use crate::{
    pipeline::event::PipelineEvent,
    util::{opt, opt::MaybeSender},
};
use reth_primitives::BlockNumber;

/// The state of the pipeline during execution.
pub(crate) struct PipelineState {
    pub(crate) events_sender: MaybeSender<PipelineEvent>,
    pub(crate) max_block: Option<BlockNumber>,
    /// The maximum progress achieved by any stage during the execution of the pipeline.
    pub(crate) maximum_progress: Option<BlockNumber>,
    /// The minimum progress achieved by any stage during the execution of the pipeline.
    pub(crate) minimum_progress: Option<BlockNumber>,
}

impl PipelineState {
    /// Record the progress of a stage, setting the maximum and minimum progress achieved by any
    /// stage during the execution of the pipeline.
    pub(crate) fn record_progress_outliers(&mut self, stage_progress: BlockNumber) {
        // Update our minimum and maximum stage progress
        self.minimum_progress = opt::min(self.minimum_progress, stage_progress);
        self.maximum_progress = opt::max(self.maximum_progress, stage_progress);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_progress_outliers() {
        let mut state = PipelineState {
            events_sender: MaybeSender::new(None),
            max_block: None,
            maximum_progress: None,
            minimum_progress: None,
        };

        state.record_progress_outliers(10);
        assert_eq!(state.minimum_progress, Some(10));
        assert_eq!(state.maximum_progress, Some(10));

        state.record_progress_outliers(20);
        assert_eq!(state.minimum_progress, Some(10));
        assert_eq!(state.maximum_progress, Some(20));

        state.record_progress_outliers(1);
        assert_eq!(state.minimum_progress, Some(1));
        assert_eq!(state.maximum_progress, Some(20));
    }
}
