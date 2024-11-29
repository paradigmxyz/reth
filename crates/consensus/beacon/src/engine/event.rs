use alloy_consensus::BlockHeader;
use alloy_primitives::B256;
use alloy_rpc_types_engine::ForkchoiceState;
use reth_engine_primitives::ForkchoiceStatus;
use reth_primitives::{EthPrimitives, NodePrimitives, SealedBlock, SealedHeader};
use std::{
    fmt::{Display, Formatter, Result},
    sync::Arc,
    time::Duration,
};

/// Events emitted by [`crate::BeaconConsensusEngine`].
#[derive(Clone, Debug)]
pub enum BeaconConsensusEngineEvent<N: NodePrimitives = EthPrimitives> {
    /// The fork choice state was updated, and the current fork choice status
    ForkchoiceUpdated(ForkchoiceState, ForkchoiceStatus),
    /// A block was added to the fork chain.
    ForkBlockAdded(Arc<SealedBlock<N::BlockHeader, N::BlockBody>>, Duration),
    /// A block was added to the canonical chain, and the elapsed time validating the block
    CanonicalBlockAdded(Arc<SealedBlock<N::BlockHeader, N::BlockBody>>, Duration),
    /// A canonical chain was committed, and the elapsed time committing the data
    CanonicalChainCommitted(Box<SealedHeader<N::BlockHeader>>, Duration),
    /// The consensus engine is involved in live sync, and has specific progress
    LiveSyncProgress(ConsensusEngineLiveSyncProgress),
}

impl<N: NodePrimitives> BeaconConsensusEngineEvent<N> {
    /// Returns the canonical header if the event is a
    /// [`BeaconConsensusEngineEvent::CanonicalChainCommitted`].
    pub const fn canonical_header(&self) -> Option<&SealedHeader<N::BlockHeader>> {
        match self {
            Self::CanonicalChainCommitted(header, _) => Some(header),
            _ => None,
        }
    }
}

impl<N> Display for BeaconConsensusEngineEvent<N>
where
    N: NodePrimitives<BlockHeader: BlockHeader>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::ForkchoiceUpdated(state, status) => {
                write!(f, "ForkchoiceUpdated({state:?}, {status:?})")
            }
            Self::ForkBlockAdded(block, duration) => {
                write!(f, "ForkBlockAdded({:?}, {duration:?})", block.num_hash())
            }
            Self::CanonicalBlockAdded(block, duration) => {
                write!(f, "CanonicalBlockAdded({:?}, {duration:?})", block.num_hash())
            }
            Self::CanonicalChainCommitted(block, duration) => {
                write!(f, "CanonicalChainCommitted({:?}, {duration:?})", block.num_hash())
            }
            Self::LiveSyncProgress(progress) => {
                write!(f, "LiveSyncProgress({progress:?})")
            }
        }
    }
}

/// Progress of the consensus engine during live sync.
#[derive(Clone, Debug)]
pub enum ConsensusEngineLiveSyncProgress {
    /// The consensus engine is downloading blocks from the network.
    DownloadingBlocks {
        /// The number of blocks remaining to download.
        remaining_blocks: u64,
        /// The target block hash and number to download.
        target: B256,
    },
}
