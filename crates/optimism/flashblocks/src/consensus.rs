use crate::FlashBlockCompleteSequenceRx;
use alloy_primitives::B256;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_optimism_payload_builder::OpPayloadTypes;
use reth_payload_primitives::EngineApiMessageVersion;
use tracing::*;

/// Consensus client that sends FCUs and new payloads using blocks from a [`FlashBlockService`]
///
/// [`FlashBlockService`]: crate::FlashBlockService
#[derive(Debug)]
pub struct FlashBlockConsensusClient {
    /// Handle to execution client.
    engine_handle: ConsensusEngineHandle<OpPayloadTypes>,
    sequence_receiver: FlashBlockCompleteSequenceRx,
}

impl FlashBlockConsensusClient {
    /// Create a new `FlashBlockConsensusClient` with the given Op engine and sequence receiver.
    pub const fn new(
        engine_handle: ConsensusEngineHandle<OpPayloadTypes>,
        sequence_receiver: FlashBlockCompleteSequenceRx,
    ) -> eyre::Result<Self> {
        Ok(Self { engine_handle, sequence_receiver })
    }

    /// Spawn the client to start sending FCUs and new payloads by periodically fetching recent
    /// blocks.
    pub async fn run(mut self) {
        loop {
            match self.sequence_receiver.recv().await {
                Ok(sequence) => {
                    // Extract block information for logging
                    let last_flashblock = sequence.last();
                    let block_hash = last_flashblock.diff.block_hash;
                    let block_number = sequence.payload_base().block_number;
                    let tx_count = last_flashblock.diff.transactions.len();
                    let gas_used = last_flashblock.diff.gas_used;

                    // Convert the flashblock sequence to execution payload
                    let payload = sequence.to_execution_data();

                    // Submit new payload to the engine
                    match self.engine_handle.new_payload(payload).await {
                        Ok(result) => {
                            debug!(
                                target: "optimism::flashblocks::consensus::flashblock-client",
                                flashblock_count = sequence.count(),
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
                                tx_count,
                                gas_used,
                                "failed to submit new payload"
                            );
                        }
                    }

                    // Update fork choice to make this block the head
                    let forkchoice_state = alloy_rpc_types_engine::ForkchoiceState {
                        head_block_hash: block_hash,
                        safe_block_hash: B256::ZERO,
                        finalized_block_hash: B256::ZERO,
                    };

                    match self
                        .engine_handle
                        .fork_choice_updated(forkchoice_state, None, EngineApiMessageVersion::V3)
                        .await
                    {
                        Ok(result) => {
                            debug!(
                                target: "optimism::flashblocks::consensus::flashblock-client",
                                block_number,
                                %block_hash,
                                head_block_hash = %forkchoice_state.head_block_hash,
                                safe_block_hash = %forkchoice_state.safe_block_hash,
                                finalized_block_hash = %forkchoice_state.finalized_block_hash,
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
                                head_block_hash = %forkchoice_state.head_block_hash,
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
