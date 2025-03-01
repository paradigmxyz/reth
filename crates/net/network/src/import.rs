//! This module provides an abstraction over block import in the form of the `BlockImport` trait.

use crate::message::NewBlockMessage;
use alloy_rpc_types::engine::{PayloadStatus, PayloadStatusEnum};
use reth_engine_primitives::{BeaconConsensusEngineHandle, BeaconOnNewPayloadError, EngineTypes};
use reth_network_peers::PeerId;
use reth_payload_primitives::BuiltPayload;
use reth_primitives::NodePrimitives;
use reth_primitives_traits::Block;
use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Abstraction over block import.
pub trait BlockImport<B = reth_ethereum_primitives::Block>: std::fmt::Debug + Send + Sync {
    /// Invoked for a received `NewBlock` broadcast message from the peer.
    ///
    /// > When a `NewBlock` announcement message is received from a peer, the client first verifies
    /// > the basic header validity of the block, checking whether the proof-of-work value is valid.
    ///
    /// This is supposed to start verification. The results are then expected to be returned via
    /// [`BlockImport::poll`].
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockMessage<B>);

    /// Returns the results of a [`BlockImport::on_new_block`]
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BlockImportOutcome<B>>;
}

/// Outcome of the [`BlockImport`]'s block handling.
#[derive(Debug)]
pub struct BlockImportOutcome<B = reth_ethereum_primitives::Block> {
    /// Sender of the `NewBlock` message.
    pub peer: PeerId,
    /// The result after validating the block
    pub result: Result<BlockValidation<B>, BlockImportError>,
}

/// Represents the successful validation of a received `NewBlock` message.
#[derive(Debug)]
pub enum BlockValidation<B> {
    /// Basic Header validity check, after which the block should be relayed to peers via a
    /// `NewBlock` message
    ValidHeader {
        /// received block
        block: NewBlockMessage<B>,
    },
    /// Successfully imported: state-root matches after execution. The block should be relayed via
    /// `NewBlockHashes`
    ValidBlock {
        /// validated block.
        block: NewBlockMessage<B>,
    },
}

/// Represents the error case of a failed block import
#[derive(Debug, thiserror::Error)]
pub enum BlockImportError {
    /// Consensus error
    #[error(transparent)]
    Consensus(#[from] reth_consensus::ConsensusError),
    /// Other error
    #[error(transparent)]
    Other(#[from] Box<dyn Error>),
}

/// An implementation of `BlockImport` used in Proof-of-Stake consensus that does nothing.
///
/// Block propagation over devp2p is invalid in POS: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProofOfStakeBlockImport;

impl<B> BlockImport<B> for ProofOfStakeBlockImport {
    fn on_new_block(&mut self, _peer_id: PeerId, _incoming_block: NewBlockMessage<B>) {}

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockImportOutcome<B>> {
        Poll::Pending
    }
}

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
                                Err(BlockImportError::Other(validation_error.into()))
                            }
                            PayloadStatusEnum::Syncing => {
                                Err(BlockImportError::Other("Syncing".into()))
                            }
                            _ => continue,
                        },
                    });
                }
                Poll::Ready((Err(err), peer_id, _)) => {
                    return Poll::Ready(BlockImportOutcome {
                        peer: peer_id,
                        result: Err(BlockImportError::Other(err.into())),
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
