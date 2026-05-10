//! Shared types for blockchain tree validation.

use crate::tree::error::InsertPayloadError;
use reth_chain_state::{ExecutedBlock, ExecutionTimingStats};
use reth_primitives_traits::{BlockTy, NodePrimitives};

/// Result of block or payload validation.
pub type ValidationOutcome<N, E = InsertPayloadError<BlockTy<N>>> = Result<ValidationOutput<N>, E>;

/// Result type for block validation with optional timing stats.
pub(crate) type InsertPayloadResult<N> =
    Result<ValidationOutput<N>, InsertPayloadError<<N as NodePrimitives>::Block>>;

/// Output of block or payload validation.
#[derive(Clone, Debug)]
pub struct ValidationOutput<N: NodePrimitives> {
    /// The executed block produced by validation.
    pub executed_block: ExecutedBlock<N>,
    /// Optional execution timing stats collected during validation.
    pub execution_timing_stats: Option<Box<ExecutionTimingStats>>,
}

impl<N: NodePrimitives> ValidationOutput<N> {
    /// Creates a new validation output.
    pub const fn new(
        executed_block: ExecutedBlock<N>,
        execution_timing_stats: Option<Box<ExecutionTimingStats>>,
    ) -> Self {
        Self { executed_block, execution_timing_stats }
    }
}
