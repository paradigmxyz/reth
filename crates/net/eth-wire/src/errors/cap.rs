use reth_primitives::BytesMut;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::{capability::SharedCapabilityError, sharedstream::CapStreamMsg};

use super::P2PStreamError;

#[derive(Debug, Error)]
pub enum CapStreamError {
    #[error(transparent)]
    P2P(#[from] P2PStreamError),
    #[error(transparent)]
    ShareP2P(#[from] SharedStreamError),
}

#[derive(Debug, Error)]
pub enum SharedStreamError {
    #[error(transparent)]
    P2PStreamError(#[from] P2PStreamError),
    #[error("capability not recognized")]
    CapabilityNotRecognized,
    #[error("capability not configured in app")]
    CapabilityNotConfigured,
    #[error("unable to share stream for capability not supported on stream to this peer")]
    CapabilityNotConfigurable,
    #[error("unknown capability message id: {0}")]
    UnknownCapabilityMessageId(u8),
    #[error(transparent)]
    ParseSharedCapability(#[from] SharedCapabilityError),
    #[error("failed to clone shared stream, {0}")]
    RecvClonedStreamFailed(#[from] oneshot::error::RecvError),
    #[error("failed to return shared stream clone")]
    SendClonedStreamFailed,
    #[error("failed to receive ingress bytes, channel closed")]
    RecvIngressBytesFailed,
    #[error("failed to stream ingress bytes, {0}")]
    SendIngressBytesFailed(mpsc::error::SendError<BytesMut>),
    #[error("weak cap stream clone failed to sink message to cap stream, {0}")]
    WeakCloneSendEgressFailed(mpsc::error::SendError<CapStreamMsg>),
    #[error("shared stream already cloned for capability")]
    SharedStreamExists,
    #[error("shared stream in use by clone")]
    SharedStreamInUse,
    #[error("clones not permitted to disconnect shared stream")]
    NoDisconnectByClone,
}
