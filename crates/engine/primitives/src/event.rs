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
pub use reth_chain_state::BlockValidationTiming;
use reth_chain_state::ExecutedBlock;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{NodePrimitives, SealedBlock, SealedHeader};

/// Type alias for backwards compat
#[deprecated(note = "Use ConsensusEngineEvent instead")]
pub type BeaconConsensusEngineEvent<N> = ConsensusEngineEvent<N>;

/// Events emitted by the consensus engine.
#[derive(Clone, Debug)]
pub enum ConsensusEngineEvent<N: NodePrimitives = EthPrimitives> {
    /// The fork choice state was updated, and the current fork choice status
    ForkchoiceUpdated(ForkchoiceState, ForkchoiceStatus),
    /// A block was added to the fork chain.
    ForkBlockAdded(ExecutedBlock<N>, Duration),
    /// A new block was received from the consensus engine
    BlockReceived(BlockNumHash),
    /// A block was added to the canonical chain, with timing information for validation.
    CanonicalBlockAdded(ExecutedBlock<N>, BlockValidationTiming),
    /// A canonical chain was committed, and the elapsed time committing the data
    CanonicalChainCommitted(Box<SealedHeader<N::BlockHeader>>, Duration),
    /// The consensus engine processed an invalid block.
    InvalidBlock(Box<SealedBlock<N::Block>>),
}

impl<N: NodePrimitives> ConsensusEngineEvent<N> {
    /// Returns the canonical header if the event is a
    /// [`ConsensusEngineEvent::CanonicalChainCommitted`].
    pub const fn canonical_header(&self) -> Option<&SealedHeader<N::BlockHeader>> {
        match self {
            Self::CanonicalChainCommitted(header, _) => Some(header),
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
            Self::CanonicalBlockAdded(block, timing) => {
                write!(
                    f,
                    "CanonicalBlockAdded({:?}, exec={:?}, state_root={:?})",
                    block.recovered_block.num_hash(),
                    timing.execution_elapsed,
                    timing.state_root_elapsed,
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
        }
    }
}
