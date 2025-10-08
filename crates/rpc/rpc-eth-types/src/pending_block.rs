//! Helper types for `reth_rpc_eth_api::EthApiServer` implementation.
//!
//! Types used in block building.

use std::{sync::Arc, time::Instant};

use crate::block::BlockAndReceipts;
use alloy_consensus::BlockHeader;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{BlockHash, B256};
use derive_more::Constructor;
use reth_chain_state::{
    BlockState, ExecutedBlock, ExecutedBlockWithTrieUpdates, ExecutedTrieUpdates,
};
use reth_ethereum_primitives::Receipt;
use reth_evm::EvmEnv;
use reth_primitives_traits::{
    Block, BlockTy, NodePrimitives, ReceiptTy, RecoveredBlock, SealedHeader,
};

/// Configured [`EvmEnv`] for a pending block.
#[derive(Debug, Clone, Constructor)]
pub struct PendingBlockEnv<B: Block, R, Spec> {
    /// Configured [`EvmEnv`] for the pending block.
    pub evm_env: EvmEnv<Spec>,
    /// Origin block for the config
    pub origin: PendingBlockEnvOrigin<B, R>,
}

/// The origin for a configured [`PendingBlockEnv`]
#[derive(Clone, Debug)]
pub enum PendingBlockEnvOrigin<B: Block = reth_ethereum_primitives::Block, R = Receipt> {
    /// The pending block as received from the CL.
    ActualPending(Arc<RecoveredBlock<B>>, Arc<Vec<R>>),
    /// The _modified_ header of the latest block.
    ///
    /// This derives the pending state based on the latest header by modifying:
    ///  - the timestamp
    ///  - the block number
    ///  - fees
    DerivedFromLatest(SealedHeader<B::Header>),
}

impl<B: Block, R> PendingBlockEnvOrigin<B, R> {
    /// Returns true if the origin is the actual pending block as received from the CL.
    pub const fn is_actual_pending(&self) -> bool {
        matches!(self, Self::ActualPending(_, _))
    }

    /// Consumes the type and returns the actual pending block.
    pub fn into_actual_pending(self) -> Option<Arc<RecoveredBlock<B>>> {
        match self {
            Self::ActualPending(block, _) => Some(block),
            _ => None,
        }
    }

    /// Returns the [`BlockId`] that represents the state of the block.
    ///
    /// If this is the actual pending block, the state is the "Pending" tag, otherwise we can safely
    /// identify the block by its hash (latest block).
    pub fn state_block_id(&self) -> BlockId {
        match self {
            Self::ActualPending(_, _) => BlockNumberOrTag::Pending.into(),
            Self::DerivedFromLatest(latest) => BlockId::Hash(latest.hash().into()),
        }
    }

    /// Returns the hash of the block the pending block should be built on.
    ///
    /// For the [`PendingBlockEnvOrigin::ActualPending`] this is the parent hash of the block.
    /// For the [`PendingBlockEnvOrigin::DerivedFromLatest`] this is the hash of the _latest_
    /// header.
    pub fn build_target_hash(&self) -> B256 {
        match self {
            Self::ActualPending(block, _) => block.header().parent_hash(),
            Self::DerivedFromLatest(latest) => latest.hash(),
        }
    }
}

/// A type alias for a pair of an [`Arc`] wrapped [`RecoveredBlock`] and a vector of
/// [`NodePrimitives::Receipt`].
pub type PendingBlockAndReceipts<N> = BlockAndReceipts<N>;

/// Locally built pending block for `pending` tag.
#[derive(Debug, Clone, Constructor)]
pub struct PendingBlock<N: NodePrimitives> {
    /// Timestamp when the pending block is considered outdated.
    pub expires_at: Instant,
    /// The receipts for the pending block
    pub receipts: Arc<Vec<ReceiptTy<N>>>,
    /// The locally built pending block with execution output.
    pub executed_block: ExecutedBlock<N>,
}

impl<N: NodePrimitives> PendingBlock<N> {
    /// Creates a new instance of [`PendingBlock`] with `executed_block` as its output that should
    /// not be used past `expires_at`.
    pub fn with_executed_block(expires_at: Instant, executed_block: ExecutedBlock<N>) -> Self {
        Self {
            expires_at,
            receipts: Arc::new(
                executed_block.execution_output.receipts.iter().flatten().cloned().collect(),
            ),
            executed_block,
        }
    }

    /// Returns the locally built pending [`RecoveredBlock`].
    pub const fn block(&self) -> &Arc<RecoveredBlock<BlockTy<N>>> {
        &self.executed_block.recovered_block
    }

    /// Converts this [`PendingBlock`] into a pair of [`RecoveredBlock`] and a vector of
    /// [`NodePrimitives::Receipt`]s, taking self.
    pub fn into_block_and_receipts(self) -> PendingBlockAndReceipts<N> {
        BlockAndReceipts { block: self.executed_block.recovered_block, receipts: self.receipts }
    }

    /// Returns a pair of [`RecoveredBlock`] and a vector of  [`NodePrimitives::Receipt`]s by
    /// cloning from borrowed self.
    pub fn to_block_and_receipts(&self) -> PendingBlockAndReceipts<N> {
        BlockAndReceipts {
            block: self.executed_block.recovered_block.clone(),
            receipts: self.receipts.clone(),
        }
    }

    /// Returns a hash of the parent block for this `executed_block`.
    pub fn parent_hash(&self) -> BlockHash {
        self.executed_block.recovered_block().parent_hash()
    }
}

impl<N: NodePrimitives> From<PendingBlock<N>> for BlockState<N> {
    fn from(pending_block: PendingBlock<N>) -> Self {
        Self::new(ExecutedBlockWithTrieUpdates::<N>::new(
            pending_block.executed_block.recovered_block,
            pending_block.executed_block.execution_output,
            pending_block.executed_block.hashed_state,
            ExecutedTrieUpdates::Missing,
        ))
    }
}
