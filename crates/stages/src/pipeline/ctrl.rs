use reth_primitives::BlockNumber;

/// Determines the control flow during pipeline execution.
#[derive(Debug, Eq, PartialEq)]
pub enum ControlFlow {
    /// An unwind was requested and must be performed before continuing.
    Unwind {
        /// The block to unwind to.
        target: BlockNumber,
        /// The block that caused the unwind.
        bad_block: Option<BlockNumber>,
    },
    /// The pipeline is allowed to continue executing stages.
    Continue {
        /// The progress of the last stage
        progress: BlockNumber,
    },
    /// Pipeline made no progress
    NoProgress {
        /// The current stage progress.
        stage_progress: Option<BlockNumber>,
    },
}

impl ControlFlow {
    /// Whether the pipeline should continue executing stages.
    pub fn should_continue(&self) -> bool {
        matches!(self, ControlFlow::Continue { .. } | ControlFlow::NoProgress { .. })
    }

    /// Returns true if the control flow is unwind.
    pub fn is_unwind(&self) -> bool {
        matches!(self, ControlFlow::Unwind { .. })
    }
}
