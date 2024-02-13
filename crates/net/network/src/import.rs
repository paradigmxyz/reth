//! This module provides an abstraction over block import in the form of the `BlockImport` trait.

use crate::message::NewBlockMessage;
use futures::Stream;
use reth_primitives::PeerId;
use std::task::{Context, Poll};

/// Abstraction over block import.
pub trait BlockImport:
    std::fmt::Debug + Send + Sync + Stream<Item = BlockImportOutcome> + Unpin
{
    /// Invoked for a received `NewBlock` broadcast message from the peer.
    ///
    /// > When a `NewBlock` announcement message is received from a peer, the client first verifies
    /// > the basic header validity of the block, checking whether the proof-of-work value is valid.
    ///
    /// This is supposed to start verification. The results are then expected to be returned via
    /// [`BlockImport::poll`].
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockMessage);
}

/// Outcome of the [`BlockImport`]'s block handling.
#[derive(Debug)]
pub struct BlockImportOutcome {
    /// Sender of the `NewBlock` message.
    pub peer: PeerId,
    /// The result after validating the block
    pub result: Result<BlockValidation, BlockImportError>,
}

/// Represents the successful validation of a received `NewBlock` message.
#[derive(Debug)]
pub enum BlockValidation {
    /// Basic Header validity check, after which the block should be relayed to peers via a
    /// `NewBlock` message
    ValidHeader {
        /// received block
        block: NewBlockMessage,
    },
    /// Successfully imported: state-root matches after execution. The block should be relayed via
    /// `NewBlockHashes`
    ValidBlock {
        /// validated block.
        block: NewBlockMessage,
    },
}

/// Represents the error case of a failed block import
#[derive(Debug, thiserror::Error)]
pub enum BlockImportError {
    /// Consensus error
    #[error(transparent)]
    Consensus(#[from] reth_interfaces::consensus::ConsensusError),
}

/// An implementation of `BlockImport` used in Proof-of-Stake consensus that does nothing.
///
/// Block propagation over devp2p is invalid in POS: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProofOfStakeBlockImport;

impl BlockImport for ProofOfStakeBlockImport {
    fn on_new_block(&mut self, _peer_id: PeerId, _incoming_block: NewBlockMessage) {}
}

impl Stream for ProofOfStakeBlockImport {
    type Item = BlockImportOutcome;

    /// Returns the results of a [`BlockImport::on_new_block`], i.e. nothing.
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}
