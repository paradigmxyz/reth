use crate::{
    stage::{ExecOutput, UnwindInput, UnwindOutput},
    StageCheckpoint, StageId,
};
use alloy_primitives::BlockNumber;
use std::fmt::{Display, Formatter};

/// An event emitted by a [Pipeline][crate::Pipeline].
///
/// It is possible for multiple of these events to be emitted over the duration of a pipeline's
/// execution since:
///
/// - Other stages may ask the pipeline to unwind
/// - The pipeline will loop indefinitely unless a target block is set
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PipelineEvent {
    /// Emitted when a stage is about to be prepared for a run.
    Prepare {
        /// Pipeline stages progress.
        pipeline_stages_progress: PipelineStagesProgress,
        /// The stage that is about to be run.
        stage_id: StageId,
        /// The previous checkpoint of the stage.
        checkpoint: Option<StageCheckpoint>,
        /// The block number up to which the stage is running, if known.
        target: Option<BlockNumber>,
    },
    /// Emitted when a stage is about to be run.
    Run {
        /// Pipeline stages progress.
        pipeline_stages_progress: PipelineStagesProgress,
        /// The stage that is about to be run.
        stage_id: StageId,
        /// The previous checkpoint of the stage.
        checkpoint: Option<StageCheckpoint>,
        /// The block number up to which the stage is running, if known.
        target: Option<BlockNumber>,
    },
    /// Emitted when a stage has run a single time.
    Ran {
        /// Pipeline stages progress.
        pipeline_stages_progress: PipelineStagesProgress,
        /// The stage that was run.
        stage_id: StageId,
        /// The result of executing the stage.
        result: ExecOutput,
    },
    /// Emitted when a stage is about to be unwound.
    Unwind {
        /// The stage that is about to be unwound.
        stage_id: StageId,
        /// The unwind parameters.
        input: UnwindInput,
    },
    /// Emitted when a stage has been unwound.
    Unwound {
        /// The stage that was unwound.
        stage_id: StageId,
        /// The result of unwinding the stage.
        result: UnwindOutput,
    },
    /// Emitted when a stage encounters an error either during execution or unwinding.
    Error {
        /// The stage that encountered an error.
        stage_id: StageId,
    },
    /// Emitted when a stage was skipped due to it's run conditions not being met:
    ///
    /// - The stage might have progressed beyond the point of our target block
    /// - The stage might not need to be unwound since it has not progressed past the unwind target
    /// - The stage requires that the pipeline has reached the tip, but it has not done so yet
    Skipped {
        /// The stage that was skipped.
        stage_id: StageId,
    },
}

/// Pipeline stages progress.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PipelineStagesProgress {
    /// 1-indexed ID of the stage that is about to be run out of total stages in the pipeline.
    pub current: usize,
    /// Total number of stages in the pipeline.
    pub total: usize,
}

impl Display for PipelineStagesProgress {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.current, self.total)
    }
}
