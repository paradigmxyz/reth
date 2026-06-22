//! Shared types for blockchain tree validation.

use crate::tree::error::InsertPayloadError;
use alloy_eip7928::bal::RawBal;
use alloy_primitives::B256;
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
    /// Validated raw block access list carried by the payload.
    pub raw_bal: Option<RawBal>,
    /// Block hash represented by the reusable sparse trie after validation.
    pub reusable_sparse_trie_block_hash: Option<B256>,
}

impl<N: NodePrimitives> ValidationOutput<N> {
    /// Creates a new validation output.
    pub const fn new(
        executed_block: ExecutedBlock<N>,
        execution_timing_stats: Option<Box<ExecutionTimingStats>>,
    ) -> Self {
        Self {
            executed_block,
            execution_timing_stats,
            raw_bal: None,
            reusable_sparse_trie_block_hash: None,
        }
    }

    /// Sets the validated raw block access list carried by the payload.
    pub fn with_raw_bal(mut self, raw_bal: Option<RawBal>) -> Self {
        self.raw_bal = raw_bal;
        self
    }

    /// Sets the block hash represented by the reusable sparse trie after validation.
    pub const fn with_reusable_sparse_trie_block_hash(
        mut self,
        reusable_sparse_trie_block_hash: Option<B256>,
    ) -> Self {
        self.reusable_sparse_trie_block_hash = reusable_sparse_trie_block_hash;
        self
    }
}
