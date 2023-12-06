use crate::engine::forkchoice::ForkchoiceStatus;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{SealedBlock, SealedHeader};
use std::{sync::Arc, time::Duration};

/// Events emitted by [crate::BeaconConsensusEngine].
#[derive(Clone, Debug)]
pub enum BeaconConsensusEngineEvent {
    /// The fork choice state was updated.
    ForkchoiceUpdated(ForkchoiceState, ForkchoiceStatus),
    /// A block was added to the canonical chain.
    CanonicalBlockAdded(Arc<SealedBlock>),
    /// A canonical chain was committed.
    CanonicalChainCommitted(Box<SealedHeader>, Duration),
    /// A block was added to the fork chain.
    ForkBlockAdded(Arc<SealedBlock>),
}
