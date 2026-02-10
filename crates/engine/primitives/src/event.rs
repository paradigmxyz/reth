//! Events emitted by the beacon consensus engine.

use crate::ForkchoiceStatus;
use alloc::boxed::Box;
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_rpc_types_engine::ForkchoiceState;
use core::{
    fmt::{Display, Formatter, Result},
    time::Duration,
};
use reth_chain_state::{ExecutedBlock, ExecutionTimingStats};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{NodePrimitives, SealedBlock, SealedHeader};

/// Type alias for backwards compat
#[deprecated(note = "Use ConsensusEngineEvent instead")]
pub type BeaconConsensusEngineEvent<N> = ConsensusEngineEvent<N>;

/// Information about a slow block detected after persistence.
#[derive(Clone, Debug)]
pub struct SlowBlockInfo {
    /// The timing statistics for the slow block.
    pub stats: ExecutionTimingStats,
    /// The commit duration for the batch containing this block.
    pub commit_duration: Duration,
    /// The total duration (execution + state_read + state_hash + commit).
    pub total_duration: Duration,
}

/// Events emitted by the consensus engine.
#[derive(Clone, Debug)]
pub enum ConsensusEngineEvent<N: NodePrimitives = EthPrimitives> {
    /// The fork choice state was updated, and the current fork choice status
    ForkchoiceUpdated(ForkchoiceState, ForkchoiceStatus),
    /// A block was added to the fork chain.
    ForkBlockAdded(ExecutedBlock<N>, Duration),
    /// A new block was received from the consensus engine
    BlockReceived(BlockNumHash),
    /// A block was added to the canonical chain, and the elapsed time validating the block
    CanonicalBlockAdded(ExecutedBlock<N>, Duration),
    /// A canonical chain was committed, and the elapsed time committing the data
    CanonicalChainCommitted(Box<SealedHeader<N::BlockHeader>>, Duration),
    /// The consensus engine processed an invalid block.
    InvalidBlock(Box<SealedBlock<N::Block>>),
    /// A slow block was detected after persistence, with its timing statistics.
    SlowBlock(SlowBlockInfo),
}

impl<N: NodePrimitives> ConsensusEngineEvent<N> {
    /// Returns the canonical header if the event is a
    /// [`ConsensusEngineEvent::CanonicalChainCommitted`].
    pub const fn canonical_header(&self) -> Option<&SealedHeader<N::BlockHeader>> {
        match self {
            Self::CanonicalChainCommitted(header, _) => Some(header),
            Self::SlowBlock(_) => None,
            _ => None,
        }
    }
}

impl<N> Display for ConsensusEngineEvent<N>
where
    N: NodePrimitives<BlockHeader: BlockHeader>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::ForkchoiceUpdated(state, status) => {
                write!(f, "ForkchoiceUpdated({state:?}, {status:?})")
            }
            Self::ForkBlockAdded(block, duration) => {
                write!(f, "ForkBlockAdded({:?}, {duration:?})", block.recovered_block.num_hash())
            }
            Self::CanonicalBlockAdded(block, duration) => {
                write!(
                    f,
                    "CanonicalBlockAdded({:?}, {duration:?})",
                    block.recovered_block.num_hash()
                )
            }
            Self::CanonicalChainCommitted(block, duration) => {
                write!(f, "CanonicalChainCommitted({:?}, {duration:?})", block.num_hash())
            }
            Self::InvalidBlock(block) => {
                write!(f, "InvalidBlock({:?})", block.num_hash())
            }
            Self::BlockReceived(num_hash) => {
                write!(f, "BlockReceived({num_hash:?})")
            }
            Self::SlowBlock(info) => {
                write!(
                    f,
                    "SlowBlock(block={}, total={:?})",
                    info.stats.block_number, info.total_duration
                )
            }
        }
    }
}
