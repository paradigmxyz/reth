//! This module provides an abstraction over block import in the form of the `BlockImport` trait.

use crate::message::NewBlockMessage;
use reth_eth_wire_types::broadcast::NewBlockHashes;
use reth_network_peers::PeerId;
use std::{
    error::Error,
    task::{Context, Poll},
};

/// Abstraction over block import.
pub trait BlockImport<B = reth_ethereum_primitives::Block>: std::fmt::Debug + Send + Sync {
    /// Invoked for a received block announcement from the peer.
    ///
    /// For a `NewBlock` message:
    /// > When a `NewBlock` announcement message is received from a peer, the client first verifies
    /// > the basic header validity of the block, checking whether the proof-of-work value is valid.
    ///
    /// For a `NewBlockHashes` message, hash announcement should be processed accordingly.
    ///
    /// The results are expected to be returned via [`BlockImport::poll`].
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockEvent<B>);

    /// Returns the results of a [`BlockImport::on_new_block`]
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BlockImportEvent<B>>;
}

/// Represents different types of block announcement events from the network.
#[derive(Debug, Clone)]
pub enum NewBlockEvent<B = reth_ethereum_primitives::Block> {
    /// A new full block announcement
    Block(NewBlockMessage<B>),
    /// Only the hashes of new blocks
    Hashes(NewBlockHashes),
}

/// Represents different types of block import events
#[derive(Debug)]
pub enum BlockImportEvent<B = reth_ethereum_primitives::Block> {
    /// General block announcement and validation status
    Announcement(BlockValidation<B>),
    /// Result of a peer-specific block import
    Outcome(BlockImportOutcome<B>),
}

/// Outcome of the [`BlockImport`]'s block handling.
#[derive(Debug)]
pub struct BlockImportOutcome<B = reth_ethereum_primitives::Block> {
    /// Sender of the block announcement message.
    pub peer: PeerId,
    /// The result after validating the block
    pub result: Result<BlockValidation<B>, BlockImportError>,
}

/// Represents the successful validation of a received block announcement.
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
    Other(#[from] Box<dyn Error + Send + Sync>),
}

/// An implementation of `BlockImport` used in Proof-of-Stake consensus that does nothing.
///
/// Block propagation over devp2p is invalid in POS: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProofOfStakeBlockImport;

impl<B> BlockImport<B> for ProofOfStakeBlockImport {
    fn on_new_block(&mut self, _peer_id: PeerId, _incoming_block: NewBlockEvent<B>) {}

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockImportEvent<B>> {
        Poll::Pending
    }
}
