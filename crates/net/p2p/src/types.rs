//! Common p2p types

use futures::Stream;
use reth_primitives::{Header, H256, H512};
use std::pin::Pin;

/// The stream of peer messages
pub type MessageStream = Pin<Box<dyn Stream<Item = PeerMessage> + Send>>;

/// Type alias for sentry ID
pub type SentryId = usize;
/// Type alias for node peer ID
pub type PeerId = H512;

/// The current node status
#[derive(Debug, Clone, Copy, Default)]
pub struct Status {
    /// Current block heigh
    pub height: u64,
    /// Current chain head hash
    pub hash: H256,
    /// Current total difficulty
    pub total_difficulty: H256,
}

/// The incoming peer message
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerMessage {
    /// The message content
    pub msg: Message,
    /// The peer ID
    pub peer_id: PeerId,
    /// The sentry ID
    pub sentry_id: SentryId,
}

/// Inbound message content
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// The block headers message variant.
    /// `(request_id, collection_of_headers)`
    BlockHeaders(u64, Vec<Header>),
}
