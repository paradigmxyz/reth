use crate::engine::forkchoice::ForkchoiceStatus;
use reth_interfaces::{blockchain_tree::CanonicalOutcome, consensus::ForkchoiceState};
use reth_primitives::SealedBlock;
use std::{sync::Arc, time::Duration};

/// Events emitted by [crate::BeaconConsensusEngine].
#[derive(Clone, Debug)]
pub enum BeaconConsensusEngineEvent {
    /// The fork choice state was updated.
    ForkchoiceUpdated(ForkchoiceState, ForkchoiceStatus),
    /// A block was added to the canonical chain.
    CanonicalBlockAdded(Arc<SealedBlock>),
    /// A chain was canonicalized.
    ChainCanonicalized(CanonicalOutcome, Duration),
    /// A block was added to the fork chain.
    ForkBlockAdded(Arc<SealedBlock>),
}
