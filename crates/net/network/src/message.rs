//! Capability messaging
//!
//! An RLPx stream is multiplexed via the prepended message-id of a framed message.
//! Capabilities are exchanged via the RLPx `Hello` message as pairs of `(id, version)`, <https://github.com/ethereum/devp2p/blob/master/rlpx.md#capability-messaging>

use bytes::{BufMut, Bytes};
use reth_eth_wire::{BlockHeaders, EthMessage, GetBlockHeaders};
use reth_rlp::{Decodable, DecodeError, Encodable};
use reth_rlp_derive::{RlpDecodable, RlpEncodable};
use smol_str::SmolStr;
use tokio::sync::{mpsc, oneshot};

/// Result alias for result of a request.
pub type RequestResult<T> = Result<T, RequestError>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum RequestError {
    #[error("Closed channel.")]
    ChannelClosed,
    #[error("Not connected to the node.")]
    NotConnected,
    #[error("Capability Message is not supported by remote peer.")]
    UnsupportedCapability,
    #[error("Network error: {0}")]
    Io(String),
}

impl<T> From<mpsc::error::SendError<T>> for RequestError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        RequestError::ChannelClosed
    }
}

impl From<oneshot::error::RecvError> for RequestError {
    fn from(_: oneshot::error::RecvError) -> Self {
        RequestError::ChannelClosed
    }
}

/// Represents all capabilities of a node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Capabilities {
    /// All Capabilities and their versions
    inner: Vec<Capability>,
    eth_66: bool,
    eth_67: bool,
}

impl Capabilities {
    /// Whether this peer supports eth v66 protocol.
    #[inline]
    pub fn supports_eth_v66(&self) -> bool {
        self.eth_66
    }

    /// Whether this peer supports eth v67 protocol.
    #[inline]
    pub fn supports_eth_v67(&self) -> bool {
        self.eth_67
    }
}

impl Encodable for Capabilities {
    fn encode(&self, out: &mut dyn BufMut) {
        self.inner.encode(out)
    }
}

impl Decodable for Capabilities {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let inner = Vec::<Capability>::decode(buf)?;

        Ok(Self {
            eth_66: inner.iter().any(Capability::is_eth_v66),
            eth_67: inner.iter().any(Capability::is_eth_v67),
            inner,
        })
    }
}

/// Represents an announced Capability in the `Hello` message
#[derive(Debug, Clone, Eq, PartialEq, RlpDecodable, RlpEncodable)]
pub struct Capability {
    /// Name of the Capability
    pub name: SmolStr,
    /// The version of the capability
    pub version: u64,
}

// === impl Capability ===

impl Capability {
    /// Whether this is eth v66 protocol.
    #[inline]
    pub fn is_eth_v66(&self) -> bool {
        self.name == "eth" && self.version == 66
    }

    /// Whether this is eth v67.
    #[inline]
    pub fn is_eth_v67(&self) -> bool {
        self.name == "eth" && self.version == 67
    }
}

/// A Capability message consisting of the message-id and the payload
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RawCapabilityMessage {
    /// Identifier of the message.
    pub id: usize,
    /// Actual payload
    pub payload: Bytes,
}

/// Various protocol related event types bubbled up from a session that need to be handled by the
/// network.
#[derive(Debug)]
pub enum CapabilityMessage {
    /// Eth sub-protocol message.
    Eth(EthMessage),
    /// Any other capability message.
    Other(RawCapabilityMessage),
}

/// Protocol related request messages that expect a response
#[derive(Debug)]
pub enum CapabilityRequest {
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        request: GetBlockHeaders,
        response: oneshot::Sender<RequestResult<BlockHeaders>>,
    },
}

/// The actual response object
#[derive(Debug)]
pub enum CapabilityResponse {
    GetBlockHeaders(RequestResult<BlockHeaders>),
}
