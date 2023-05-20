use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{BlockHash, BlockNumber, SealedBlock};

/// Events emitted by [crate::BeaconConsensusEngine].
#[derive(Clone, Debug)]
pub enum BeaconConsensusEngineEvent {
    /// The fork choice state was updated.
    ForkchoiceUpdated(ForkchoiceState),
    /// A block was added to the canonical chain.
    CanonicalBlockAdded(BlockNumber, BlockHash, SealedBlock),
    /// A block was added to the fork chain.
    ForkBlockAdded(BlockNumber, BlockHash, SealedBlock),
}
