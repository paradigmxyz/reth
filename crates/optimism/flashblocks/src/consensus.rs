use crate::{sequence::FlashBlockCompleteSequence, FlashBlockCompleteSequenceRx};
use reth_node_api::{ConsensusEngineHandle, EngineApiMessageVersion, PayloadTypes};
use reth_optimism_payload_builder::OpPayloadTypes;
use reth_optimism_primitives::{OpBlock, OpPrimitives};
use reth_primitives_traits::SealedBlock;
use tracing::warn;

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
                    let block_hash = sequence.last().diff.block_hash;
                    let block = self.sequence_to_block(sequence);

                    let payload = OpPayloadTypes::<OpPrimitives>::block_to_payload(
                        SealedBlock::new_unhashed(block),
                    );

                    // Send new events to execution client
                    let _ = self.engine_handle.new_payload(payload).await;

                    let state = alloy_rpc_types_engine::ForkchoiceState {
                        head_block_hash: block_hash,
                        safe_block_hash: block_hash,      // TODO
                        finalized_block_hash: block_hash, // TODO
                    };
                    let _ = self
                        .engine_handle
                        .fork_choice_updated(state, None, EngineApiMessageVersion::V3)
                        .await;
                }
                Err(err) => {
                    warn!(
                        target: "consensus::flashblock-client",
                        %err,
                        "error while fetching flashblock completed sequence"
                    );
                    break;
                }
            }
        }
    }

    fn sequence_to_block(&self, _block: FlashBlockCompleteSequence) -> OpBlock {
        unimplemented!()
    }
}
