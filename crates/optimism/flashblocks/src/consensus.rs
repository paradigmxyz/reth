use crate::{FlashBlockCompleteSequence, FlashBlockCompleteSequenceRx};
use alloy_primitives::B256;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_optimism_payload_builder::OpPayloadTypes;
use reth_payload_primitives::{EngineApiMessageVersion, PayloadTypes};
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
    P::ExecutionData: for<'a> From<&'a FlashBlockCompleteSequence>,
{
    /// Create a new `FlashBlockConsensusClient` with the given Op engine and sequence receiver.
    pub const fn new(
        engine_handle: ConsensusEngineHandle<P>,
        sequence_receiver: FlashBlockCompleteSequenceRx,
    ) -> eyre::Result<Self> {
        Ok(Self { engine_handle, sequence_receiver })
    }

    /// Run the client to process flashblock sequences and submit them to the consensus engine.
    pub async fn run(mut self) {
        loop {
            match self.sequence_receiver.recv().await {
                Ok(sequence) => {
                    // Extract block information for logging
                    let last_flashblock = sequence.last();
                    let flashblock_count = sequence.count();
                    let block_number = sequence.payload_base().block_number;
                    let block_hash = last_flashblock.diff.block_hash;
                    let gas_used = last_flashblock.diff.gas_used;

                    // Convert the flashblock sequence to execution payload
                    let payload = P::ExecutionData::from(&sequence);

                    // Submit new payload to the engine
                    match self.engine_handle.new_payload(payload).await {
                        Ok(result) => {
                            debug!(
                                target: "optimism::flashblocks::consensus::flashblock-client",
                                flashblock_count,
                                block_number,
                                %block_hash,
                                gas_used,
                                ?result,
                                "successfully submitted new payload"
                            );
                        }
                        Err(err) => {
                            warn!(
                                target: "optimism::flashblocks::consensus::flashblock-client",
                                %err,
                                block_number,
                                %block_hash,
                                gas_used,
                                "failed to submit new payload"
                            );
                            return;
                        }
                    }

                    // Update fork choice to make this block the head, only after new_payload
                    // succeeded.
                    let fcu_state = alloy_rpc_types_engine::ForkchoiceState {
                        head_block_hash: block_hash,
                        safe_block_hash: B256::ZERO,
                        finalized_block_hash: B256::ZERO,
                    };

                    match self
                        .engine_handle
                        .fork_choice_updated(fcu_state, None, EngineApiMessageVersion::V3)
                        .await
                    {
                        Ok(result) => {
                            debug!(
                                target: "optimism::flashblocks::consensus::flashblock-client",
                                block_number,
                                %block_hash,
                                head_block_hash = %fcu_state.head_block_hash,
                                ?result,
                                "successfully updated fork choice"
                            );
                        }
                        Err(err) => {
                            warn!(
                                target: "optimism::flashblocks::consensus::flashblock-client",
                                %err,
                                block_number,
                                %block_hash,
                                head_block_hash = %fcu_state.head_block_hash,
                                "failed to submit fork choice update"
                            );
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        target: "optimism::flashblocks::consensus::flashblock-client",
                        %err,
                        "error while fetching flashblock completed sequence"
                    );
                    break;
                }
            }
        }
    }
}
