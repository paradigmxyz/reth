use crate::PrunerEvent;
use reth_db::DatabaseError;
use reth_interfaces::RethError;
use reth_network_api::NetworkError;
use reth_primitives::PruneSegmentError;
use reth_provider::ProviderError;
use thiserror::Error;
use tokio::sync::broadcast::error::SendError;

#[derive(Error, Debug)]
pub enum PrunerError {
    #[error(transparent)]
    PruneSegment(#[from] PruneSegmentError),

    #[error("inconsistent data: {0}")]
    InconsistentData(&'static str),

    #[error(transparent)]
    Interface(#[from] RethError),

    #[error(transparent)]
    Database(#[from] DatabaseError),

    #[error(transparent)]
    Provider(#[from] ProviderError),
}

impl From<PrunerError> for RethError {
    fn from(err: PrunerError) -> Self {
        match err {
            PrunerError::PruneSegment(_) | PrunerError::InconsistentData(_) => {
                RethError::Custom(err.to_string())
            }
            PrunerError::Interface(err) => err,
            PrunerError::Database(err) => RethError::Database(err),
            PrunerError::Provider(err) => RethError::Provider(err),
        }
    }
}

impl From<SendError<PrunerEvent>> for PrunerError {
    fn from(_: SendError<PrunerEvent>) -> Self {
        Self::Interface(RethError::Network(NetworkError::ChannelClosed))
    }
}
