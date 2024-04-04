//! Optimism specific types.

use crate::{
    eth::error::{EthApiError, ToRpcError},
    result::internal_rpc_err,
};
use jsonrpsee::types::ErrorObject;
use reqwest::Client;

/// Eth Optimism Api Error
#[cfg(feature = "optimism")]
#[derive(Debug, thiserror::Error)]
pub enum OptimismEthApiError {
    /// Wrapper around a [hyper::Error].
    #[error(transparent)]
    HyperError(#[from] hyper::Error),
    /// Wrapper around an [reqwest::Error].
    #[error(transparent)]
    HttpError(#[from] reqwest::Error),
    /// Thrown when serializing transaction to forward to sequencer
    #[error("invalid sequencer transaction")]
    InvalidSequencerTransaction,
    /// Thrown when calculating L1 gas fee
    #[error("failed to calculate l1 gas fee")]
    L1BlockFeeError,
    /// Thrown when calculating L1 gas used
    #[error("failed to calculate l1 gas used")]
    L1BlockGasError,
}

impl ToRpcError for OptimismEthApiError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        match self {
            OptimismEthApiError::HyperError(err) => internal_rpc_err(err.to_string()),
            OptimismEthApiError::HttpError(err) => internal_rpc_err(err.to_string()),
            OptimismEthApiError::InvalidSequencerTransaction |
            OptimismEthApiError::L1BlockFeeError |
            OptimismEthApiError::L1BlockGasError => internal_rpc_err(self.to_string()),
        }
    }
}

impl From<OptimismEthApiError> for EthApiError {
    fn from(err: OptimismEthApiError) -> Self {
        EthApiError::other(err)
    }
}

/// A client to interact with a Sequencer
#[derive(Debug, Default, Clone)]
pub struct SequencerClient {
    /// The endpoint of the sequencer
    pub sequencer_endpoint: String,
    /// The HTTP client
    pub http_client: Client,
}

impl SequencerClient {
    /// Creates a new [SequencerClient].
    pub fn new(sequencer_endpoint: impl Into<String>, http_client: Client) -> Self {
        Self { sequencer_endpoint: sequencer_endpoint.into(), http_client }
    }
}
