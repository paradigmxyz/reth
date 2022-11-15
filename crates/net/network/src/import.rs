use crate::message::NewBlockMessage;
use reth_primitives::PeerId;
use std::task::{Context, Poll};

/// Abstraction over block import.
pub trait BlockImport: Send + Sync {
    /// Invoked for a received `NewBlock` broadcast message from the peer.
    ///
    /// > When a `NewBlock` announcement message is received from a peer, the client first verifies
    /// > the basic header validity of the block, checking whether the proof-of-work value is valid.
    ///
    /// This is supposed to start verification. The results are then expected to be returned via
    /// [`BlockImport::poll`].
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockMessage);

    /// Returns the results of a [`BlockImport::on_new_block`]
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BlockImportOutcome>;
}

/// Outcome of the [`BlockImport`]'s block handling.
pub struct BlockImportOutcome {
    /// Sender of the `NewBlock` message.
    pub peer: PeerId,
    /// The result after validating the block
    pub result: Result<NewBlockMessage, BlockImportError>,
}

/// Represents the error case of a failed block import
pub enum BlockImportError {}

/// An implementation of `BlockImport` that does nothing
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopBlockImport;

impl BlockImport for NoopBlockImport {
    fn on_new_block(&mut self, _peer_id: PeerId, _incoming_block: NewBlockMessage) {}

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockImportOutcome> {
        Poll::Pending
    }
}
