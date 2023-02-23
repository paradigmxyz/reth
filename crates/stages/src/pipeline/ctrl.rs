use reth_primitives::BlockNumber;

/// Determines the control flow during pipeline execution.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ControlFlow {
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
    NoProgress {
        /// The current stage progress.
        stage_progress: Option<BlockNumber>,
    },
}

impl ControlFlow {
    pub(crate) fn should_continue(&self) -> bool {
        matches!(self, ControlFlow::Continue { .. } | ControlFlow::NoProgress { .. })
    }
}
