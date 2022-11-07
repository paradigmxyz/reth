//! Capability messaging
//!
//! An RLPx stream is multiplexed via the prepended message-id of a framed message.
//! Capabilities are exchanged via the RLPx `Hello` message as pairs of `(id, version)`, <https://github.com/ethereum/devp2p/blob/master/rlpx.md#capability-messaging>

use reth_eth_wire::{BlockHeaders, GetBlockHeaders};

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
