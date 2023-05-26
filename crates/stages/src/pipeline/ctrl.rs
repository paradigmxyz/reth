use reth_primitives::{BlockNumber, SealedHeader};

/// Determines the control flow during pipeline execution.
#[derive(Debug, Eq, PartialEq)]
pub enum ControlFlow {
    /// An unwind was requested and must be performed before continuing.
    Unwind {
        /// The block to unwind to.
        target: BlockNumber,
        /// The block that caused the unwind.
        bad_block: SealedHeader,
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

    /// Returns the pipeline progress, if the state is not `Unwind`.
    pub fn progress(&self) -> Option<BlockNumber> {
        match self {
            ControlFlow::Unwind { .. } => None,
            ControlFlow::Continue { progress } => Some(*progress),
            ControlFlow::NoProgress { stage_progress } => *stage_progress,
        }
    }
}
