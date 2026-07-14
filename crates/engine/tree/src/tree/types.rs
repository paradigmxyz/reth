//! Shared types for blockchain tree validation.

use crate::tree::{error::InsertPayloadError, TxPoolPrewarmCacheSnapshot, TxPoolPrewarmHints};
use alloy_eip7928::bal::{DecodedBal, RawBal};
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::B256;
use reth_chain_state::{ExecutedBlock, ExecutionTimingStats};
use reth_evm::{ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::{BlockTy, NodePrimitives};
use std::sync::Arc;

/// EVM context required to execute a block.
#[derive(Debug, Clone)]
pub struct ExecutionEnv<Evm: ConfigureEvm> {
    /// Evm environment.
    pub evm_env: EvmEnvFor<Evm>,
    /// Hash of the block being executed.
    pub hash: B256,
    /// Hash of the parent block.
    pub parent_hash: B256,
    /// State root of the parent block.
    /// Used for sparse trie continuation: if the preserved trie's anchor matches this,
    /// the trie can be reused directly.
    pub parent_state_root: B256,
    /// Number of transactions in the block.
    /// Used to determine parallel worker count for prewarming.
    pub transaction_count: usize,
    /// Total gas used by all transactions in the block.
    /// Used to adaptively select multiproof chunk size for optimal throughput.
    pub gas_used: u64,
    /// Withdrawals included in the block.
    /// Used to generate prefetch targets for withdrawal addresses.
    pub withdrawals: Option<Vec<Withdrawal>>,
    /// Optional decoded BAL for the block.
    /// Used to validate and optimize execution.
    pub decoded_bal: Option<Arc<DecodedBal>>,
    /// Latest completed txpool-prewarm snapshot for this block's parent state.
    pub txpool_snapshot: Option<TxPoolPrewarmCacheSnapshot>,
    /// Predicted changed accounts and storage slots keyed by transaction hash.
    pub txpool_hints: Option<TxPoolPrewarmHints>,
}

impl<Evm: ConfigureEvm> ExecutionEnv<Evm>
where
    EvmEnvFor<Evm>: Default,
{
    /// Creates a new [`ExecutionEnv`] with default values for testing.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn test_default() -> Self {
        Self {
            evm_env: Default::default(),
            hash: Default::default(),
            parent_hash: Default::default(),
            parent_state_root: Default::default(),
            transaction_count: 0,
            gas_used: 0,
            withdrawals: None,
            decoded_bal: None,
            txpool_snapshot: None,
            txpool_hints: None,
        }
    }
}

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
}

impl<N: NodePrimitives> ValidationOutput<N> {
    /// Creates a new validation output.
    pub const fn new(
        executed_block: ExecutedBlock<N>,
        execution_timing_stats: Option<Box<ExecutionTimingStats>>,
    ) -> Self {
        Self { executed_block, execution_timing_stats, raw_bal: None }
    }

    /// Sets the validated raw block access list carried by the payload.
    pub fn with_raw_bal(mut self, raw_bal: Option<RawBal>) -> Self {
        self.raw_bal = raw_bal;
        self
    }
}
