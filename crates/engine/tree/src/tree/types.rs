//! Shared types for blockchain tree validation.

use crate::tree::error::InsertPayloadError;
use alloy_consensus::BlockHeader;
use alloy_eip7928::{
    bal::{Bal, DecodedBal},
    BlockAccessList,
};
use alloy_eips::{eip1898::BlockWithParent, eip4895::Withdrawal, NumHash};
use alloy_primitives::B256;
use reth_chain_state::{ExecutedBlock, ExecutionTimingStats};
use reth_engine_primitives::ExecutionPayload;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{BlockBody, BlockTy, NodePrimitives, SealedBlock};

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

/// Enum representing either block or payload being validated.
#[derive(Debug, Clone)]
pub enum BlockOrPayload<T: PayloadTypes> {
    /// Payload.
    Payload(T::ExecutionData),
    /// Block.
    Block(SealedBlock<BlockTy<<T::BuiltPayload as BuiltPayload>::Primitives>>),
}

impl<T: PayloadTypes> BlockOrPayload<T> {
    /// Returns the hash of the block.
    pub fn hash(&self) -> B256 {
        match self {
            Self::Payload(payload) => payload.block_hash(),
            Self::Block(block) => block.hash(),
        }
    }

    /// Returns the number and hash of the block.
    pub fn num_hash(&self) -> NumHash {
        match self {
            Self::Payload(payload) => payload.num_hash(),
            Self::Block(block) => block.num_hash(),
        }
    }

    /// Returns the parent hash of the block.
    pub fn parent_hash(&self) -> B256 {
        match self {
            Self::Payload(payload) => payload.parent_hash(),
            Self::Block(block) => block.parent_hash(),
        }
    }

    /// Returns [`BlockWithParent`] for the block.
    pub fn block_with_parent(&self) -> BlockWithParent {
        match self {
            Self::Payload(payload) => payload.block_with_parent(),
            Self::Block(block) => block.block_with_parent(),
        }
    }

    /// Returns a string showing whether or not this is a block or payload.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Payload(_) => "payload",
            Self::Block(_) => "block",
        }
    }

    /// Returns the block access list embedded in a payload, if present.
    pub fn block_access_list(&self) -> Option<Result<BlockAccessList, alloy_rlp::Error>> {
        match self {
            Self::Payload(payload) => payload.block_access_list().map(|block_access_list| {
                alloy_rlp::decode_exact::<Bal>(block_access_list.as_ref()).map(Bal::into_inner)
            }),
            Self::Block(_) => None,
        }
    }

    /// Returns the decoded block access list, if present and successfully decoded.
    pub fn try_decoded_access_list(&self) -> Result<Option<DecodedBal>, alloy_rlp::Error> {
        match self {
            Self::Payload(payload) => payload
                .block_access_list()
                .map(|block_access_list| DecodedBal::from_rlp_bytes(block_access_list.clone()))
                .transpose(),
            Self::Block(_) => Ok(None),
        }
    }

    /// Returns the number of transactions in the payload or block.
    pub fn transaction_count(&self) -> usize
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.transaction_count(),
            Self::Block(block) => block.transaction_count(),
        }
    }

    /// Returns the withdrawals from the payload or block.
    pub fn withdrawals(&self) -> Option<&[Withdrawal]>
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.withdrawals().map(|w| w.as_slice()),
            Self::Block(block) => block.body().withdrawals().map(|w| w.as_slice()),
        }
    }

    /// Returns the total gas used by the block.
    pub fn gas_used(&self) -> u64
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.gas_used(),
            Self::Block(block) => block.gas_used(),
        }
    }

    /// Returns the gas limit used by the block.
    pub fn gas_limit(&self) -> u64
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.gas_limit(),
            Self::Block(block) => block.gas_limit(),
        }
    }
}
