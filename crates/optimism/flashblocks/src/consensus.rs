use crate::{FlashBlockCompleteSequence, FlashBlockCompleteSequenceRx};
use alloy_primitives::B256;
use alloy_rpc_types_engine::PayloadStatusEnum;
use op_alloy_rpc_types_engine::OpExecutionData;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_optimism_payload_builder::OpPayloadTypes;
use reth_payload_primitives::{EngineApiMessageVersion, ExecutionPayload, PayloadTypes};
use tracing::*;

/// Consensus client that sends FCUs and new payloads using blocks from a [`FlashBlockService`]
///
/// [`FlashBlockService`]: crate::FlashBlockService
#[derive(Debug)]
pub struct FlashBlockConsensusClient<P = OpPayloadTypes>
where
    P: PayloadTypes,
{
    /// Handle to execution client.
    engine_handle: ConsensusEngineHandle<P>,
    sequence_receiver: FlashBlockCompleteSequenceRx,
}

impl<P> FlashBlockConsensusClient<P>
where
    P: PayloadTypes,
    P::ExecutionData: for<'a> TryFrom<&'a FlashBlockCompleteSequence, Error: std::fmt::Display>,
{
    /// Create a new `FlashBlockConsensusClient` with the given Op engine and sequence receiver.
    pub const fn new(
        engine_handle: ConsensusEngineHandle<P>,
        sequence_receiver: FlashBlockCompleteSequenceRx,
    ) -> eyre::Result<Self> {
        Ok(Self { engine_handle, sequence_receiver })
    }

    /// Spawn the client to start sending FCUs and new payloads by periodically fetching recent
    /// blocks.
    pub async fn run(mut self) {
        loop {
            let sequence = match self.sequence_receiver.recv().await {
                Ok(sequence) => sequence,
                Err(err) => {
                    error!(
                        target: "optimism::flashblocks::consensus::client",
                        %err,
                        "error while fetching flashblock completed sequence",
                    );
                    continue;
                }
            };

            let payload = match P::ExecutionData::try_from(&sequence) {
                Ok(payload) => payload,
                Err(err) => {
                    error!(
                        target: "optimism::flashblocks::consensus::client",
                        %err,
                        "error while converting to payload from completed sequence",
                    );
                    continue;
                }
            };

            let block_number = payload.block_number();
            let block_hash = payload.block_hash();
            // Call engine_newPayload
            match self.engine_handle.new_payload(payload).await {
                Ok(result) => {
                    debug!(
                        target: "optimism::flashblocks::consensus::client",
                        flashblock_count = sequence.count(),
                        block_number,
                        %block_hash,
                        ?result,
                        "Submitted engine_newPayload",
                    );

                    if result.status != PayloadStatusEnum::Valid {
                        continue;
                    }
                }
                Err(err) => {
                    error!(
                        target: "optimism::flashblocks::consensus::client",
                        %err,
                        block_number,
                        "Failed to submit new payload",
                    );
                    continue;
                }
            }

            // FCU to update tip, only after successful engine_newPayload
            let fcu_state = alloy_rpc_types_engine::ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: B256::ZERO,
                finalized_block_hash: B256::ZERO,
            };
            // version shouldn't matter here since we're not passing attrs
            match self
                .engine_handle
                .fork_choice_updated(fcu_state, None, EngineApiMessageVersion::V5)
                .await
            {
                Ok(result) => {
                    debug!(
                        target: "optimism::flashblocks::consensus::client",
                        flashblock_count = sequence.count(),
                        block_number,
                        %block_hash,
                        ?result,
                        "Successfully called engine_forkChoiceUpdated",
                    )
                }
                Err(err) => {
                    error!(
                        target: "optimism::flashblocks::consensus::client",
                        %err,
                        block_number,
                        %block_hash,
                        "Failed to submit fork choice update",
                    );
                }
            }
        }
    }
}

impl From<&FlashBlockCompleteSequence> for OpExecutionData {
    fn from(sequence: &FlashBlockCompleteSequence) -> Self {
        let mut data = Self::from_flashblocks_unchecked(sequence);
        // Replace payload's state_root with the calculated one. For flashblocks, there was an
        // option to disable state root calculation for blocks, and in that case, the payload's
        // state_root will be zero, and we'll need to locally calculate state_root before
        // proceeding to call engine_newPayload.
        if let Some(execution_outcome) = sequence.execution_outcome() {
            let payload = data.payload.as_v1_mut();
            payload.state_root = execution_outcome.state_root;
            payload.block_hash = execution_outcome.block_hash;
        }
        data
    }
}
