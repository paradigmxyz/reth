use alloy_primitives;
use reth::{primitives::SealedBlockWithSenders, providers::ExecutionOutcome};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionTipState {
    pub block_number: alloy_primitives::BlockNumber,
    pub sealed_block_with_senders: SealedBlockWithSenders,
}
