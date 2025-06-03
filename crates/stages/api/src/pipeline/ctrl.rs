use alloy_eips::eip1898::BlockWithParent;
use alloy_primitives::BlockNumber;

/// Specifies the reason for an unwind operation.
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub enum UnwindReason {
    /// Unwind triggered due to a detached head, indicating a chain reorganization.
    DetachedHead,
    /// Unwind triggered due to a block validation error.
    ValidationError,
    /// Unwind triggered due to an error during block execution.
    ExecutionError,
    /// Unwind triggered due to missing static file data necessary for a stage.
    MissingStaticData,
    /// Unwind triggered for other, unspecified reasons.
    Other,
}

/// Determines the control flow during pipeline execution.
///
/// See [`Pipeline::run_loop`](crate::Pipeline::run_loop) for more information.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ControlFlow {
    /// An unwind was requested and must be performed before continuing.
    Unwind {
        /// The block to unwind to.
        ///
        /// This marks the highest block to which the stage should unwind to.
        /// For example, unwinding to block 10, should remove all data for blocks above 10 (>=11).
        target: BlockNumber,
        /// The block that caused the unwind.
        bad_block: Box<BlockWithParent>,
        /// The reason for this unwind.
        reason: UnwindReason,
    },
    /// The pipeline made progress.
    Continue {
        /// Block number reached by the stage.
        block_number: BlockNumber,
    },
    /// Pipeline made no progress
    NoProgress {
        /// Block number reached by the stage.
        block_number: Option<BlockNumber>,
    },
}

impl ControlFlow {
    /// Whether the pipeline should continue executing stages.
    pub const fn should_continue(&self) -> bool {
        matches!(self, Self::Continue { .. } | Self::NoProgress { .. })
    }

    /// Returns true if the control flow is unwind.
    pub const fn is_unwind(&self) -> bool {
        matches!(self, Self::Unwind { .. })
    }

    /// Returns the pipeline block number the stage reached, if the state is not `Unwind`.
    pub const fn block_number(&self) -> Option<BlockNumber> {
        match self {
            Self::Unwind { .. } => None,
            Self::Continue { block_number } => Some(*block_number),
            Self::NoProgress { block_number } => *block_number,
        }
    }
}
