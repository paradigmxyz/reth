use crate::engine::forkchoice::ForkchoiceStatus;
use reth_primitives::{SealedBlock, SealedHeader, B256};
use reth_rpc_types::engine::ForkchoiceState;
use std::{sync::Arc, time::Duration};

/// Events emitted by [`crate::BeaconConsensusEngine`].
#[derive(Clone, Debug)]
pub enum BeaconConsensusEngineEvent {
    /// The fork choice state was updated, and the current fork choice status
    ForkchoiceUpdated(ForkchoiceState, ForkchoiceStatus),
    /// A block was added to the canonical chain, and the elapsed time validating the block
    CanonicalBlockAdded(Arc<SealedBlock>, Duration),
    /// A canonical chain was committed, and the elapsed time committing the data
    CanonicalChainCommitted(Box<SealedHeader>, Duration),
    /// The consensus engine is involved in live sync, and has specific progress
    LiveSyncProgress(ConsensusEngineLiveSyncProgress),
    /// A block was added to the fork chain.
    ForkBlockAdded(Arc<SealedBlock>),
}

impl BeaconConsensusEngineEvent {
    /// Returns the canonical header if the event is a
    /// [`BeaconConsensusEngineEvent::CanonicalChainCommitted`].
    pub const fn canonical_header(&self) -> Option<&SealedHeader> {
        match self {
            Self::CanonicalChainCommitted(header, _) => Some(header),
            _ => None,
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
