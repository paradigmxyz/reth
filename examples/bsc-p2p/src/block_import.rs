use alloy_rpc_types::engine::{PayloadStatus, PayloadStatusEnum};
use reth_engine_primitives::{BeaconConsensusEngineHandle, BeaconOnNewPayloadError, EngineTypes};
use reth_network::{
    import::{BlockImport, BlockImportError, BlockImportOutcome, BlockValidation},
    message::NewBlockMessage,
};
use reth_network_peers::PeerId;
use reth_payload_primitives::BuiltPayload;
use reth_primitives::NodePrimitives;
use reth_primitives_traits::Block;
use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

type BlockImportFuture<Block> = Pin<
    Box<
        dyn Future<
                Output = (
                    Result<PayloadStatus, BeaconOnNewPayloadError>,
                    PeerId,
                    NewBlockMessage<Block>,
                ),
            > + Send
            + Sync
            + 'static,
    >,
>;

pub struct BscBlockImport<EngineT: EngineTypes> {
    engine_handle: BeaconConsensusEngineHandle<EngineT>,
    pending_imports: VecDeque<
        BlockImportFuture<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    >,
}

impl<EngineT: EngineTypes>
    BlockImport<<<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block>
    for BscBlockImport<EngineT>
{
    fn on_new_block(
        &mut self,
        peer_id: PeerId,
        incoming_block: NewBlockMessage<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) {
        let payload = EngineT::block_to_payload(incoming_block.block.block.clone().seal());
        let engine_handle = self.engine_handle.clone();

        let fut = Box::pin(async move {
            let result = engine_handle.new_payload(payload).await;
            (result, peer_id, incoming_block)
        });

        self.pending_imports.push_back(fut);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<
        BlockImportOutcome<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    > {
        while let Some(mut pending_import) = self.pending_imports.pop_front() {
            match pending_import.as_mut().poll(cx) {
                Poll::Ready((Ok(payload_status), peer_id, incoming_block)) => {
                    return Poll::Ready(BlockImportOutcome {
                        peer: peer_id,
                        result: match payload_status.status {
                            PayloadStatusEnum::Valid => {
                                Ok(BlockValidation::ValidBlock { block: incoming_block })
                            }
                            PayloadStatusEnum::Invalid { validation_error } => {
                                Err(BlockImportError::InvalidPayload(validation_error))
                            }
                            _ => continue,
                        },
                    });
                }
                Poll::Ready((Err(err), peer_id, _)) => {
                    return Poll::Ready(BlockImportOutcome {
                        peer: peer_id,
                        result: Err(BlockImportError::Engine(err)),
                    });
                }
                Poll::Pending => self.pending_imports.push_back(pending_import),
            }
        }

        Poll::Pending
    }
}

impl<EngineT: EngineTypes> fmt::Debug for BscBlockImport<EngineT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BscBlockImport")
            .field("pending_imports", &self.pending_imports.len())
            .finish()
    }
}
