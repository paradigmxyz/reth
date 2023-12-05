use thiserror::Error;

use crate::capability::{SharedCapabilityError, UnsupportedCapabilityError};

use super::P2PStreamError;

/// Errors thrown by de-/muxing.
#[derive(Error, Debug)]
pub enum MuxDemuxError {
    /// Error of the underlying P2P connection.
    #[error(transparent)]
    P2PStreamError(#[from] P2PStreamError),
    /// Stream is in use by secondary stream impeding disconnect.
    #[error("secondary streams are still running")]
    StreamInUse,
    /// Stream has already been set up for this capability stream type.
    #[error("stream already init for stream type")]
    StreamAlreadyExists,
    /// Capability stream type is not shared with peer on underlying p2p connection.
    #[error("stream type is not shared on this p2p connection")]
    CapabilityNotShared,
    /// Capability stream type has not been configured in [`crate::muxdemux::MuxDemuxer`].
    #[error("stream type is not configured")]
    CapabilityNotConfigured,
    /// Capability stream type has not been configured for
    /// [`crate::capability::SharedCapabilities`] type.
    #[error("stream type is not recognized")]
    CapabilityNotRecognized,
    /// Message ID is out of range.
    #[error("message id out of range, {0}")]
    MessageIdOutOfRange(u8),
    /// Demux channel failed.
    #[error("sending demuxed bytes to secondary stream failed")]
    SendIngressBytesFailed,
    /// Mux channel failed.
    #[error("sending bytes from secondary stream to mux failed")]
    SendEgressBytesFailed,
    /// Attempt to disconnect the p2p stream via a stream clone.
    #[error("secondary stream cannot disconnect p2p stream")]
    CannotDisconnectP2PStream,
    /// Shared capability error.
    #[error(transparent)]
    SharedCapabilityError(#[from] SharedCapabilityError),
    /// Capability not supported on the p2p connection.
    #[error(transparent)]
    UnsupportedCapabilityError(#[from] UnsupportedCapabilityError),
}
