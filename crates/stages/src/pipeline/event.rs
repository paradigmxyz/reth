use crate::{
    id::StageId,
    stage::{ExecOutput, UnwindInput, UnwindOutput},
};
use reth_primitives::BlockNumber;

/// An event emitted by a [Pipeline][crate::Pipeline].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PipelineEvent {
    /// Emitted when a stage is about to be run.
    Running {
        /// The stage that is about to be run.
        stage_id: StageId,
        /// The previous checkpoint of the stage.
        stage_progress: Option<BlockNumber>,
    },
    /// Emitted when a stage has run a single time.
    ///
    /// It is possible for multiple of these events to be emitted over the duration of a pipeline's
    /// execution:
    /// - If the pipeline loops, the stage will be run again at some point
    /// - If the stage exits early but has acknowledged that it is not entirely done
    Ran {
        /// The stage that was run.
        stage_id: StageId,
        /// The result of executing the stage. If it is None then an error was encountered.
        result: Option<ExecOutput>,
    },
    /// Emitted when a stage is about to be unwound.
    Unwinding {
        /// The stage that is about to be unwound.
        stage_id: StageId,
        /// The unwind parameters.
        input: UnwindInput,
    },
    /// Emitted when a stage has been unwound.
    ///
    /// It is possible for multiple of these events to be emitted over the duration of a pipeline's
    /// execution, since other stages may ask the pipeline to unwind.
    Unwound {
        /// The stage that was unwound.
        stage_id: StageId,
        /// The result of unwinding the stage. If it is None then an error was encountered.
        result: Option<UnwindOutput>,
    },
}
