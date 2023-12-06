//! Error types emitted by types or implementations of this crate.

use reth_interfaces::{provider::ProviderError, RethError};
use reth_primitives::{revm_primitives::EVMError, B256};
use reth_transaction_pool::BlobStoreError;
use tokio::sync::oneshot;

/// Possible error variants during payload building.
#[derive(Debug, thiserror::Error)]
pub enum PayloadBuilderError {
    /// Thrown whe the parent block is missing.
    #[error("missing parent block {0}")]
    MissingParentBlock(B256),
    /// An oneshot channels has been closed.
    #[error("sender has been dropped")]
    ChannelClosed,
    /// Error occurring in the blob store.
    #[error(transparent)]
    BlobStore(#[from] BlobStoreError),
    /// Other internal error
    #[error(transparent)]
    Internal(#[from] RethError),
    /// Unrecoverable error during evm execution.
    #[error("evm execution error: {0}")]
    EvmExecutionError(EVMError<ProviderError>),
    /// Thrown if the payload requests withdrawals before Shanghai activation.
    #[error("withdrawals set before Shanghai activation")]
    WithdrawalsBeforeShanghai,
    /// Optimism specific payload building errors.
    #[cfg(feature = "optimism")]
    #[error(transparent)]
    Optimism(#[from] OptimismPayloadBuilderError),
}

impl From<ProviderError> for PayloadBuilderError {
    fn from(error: ProviderError) -> Self {
        PayloadBuilderError::Internal(RethError::Provider(error))
    }
}

/// Optimism specific payload building errors.
#[cfg(feature = "optimism")]
#[derive(Debug, thiserror::Error)]
pub enum OptimismPayloadBuilderError {
    /// Thrown when a transaction fails to convert to a
    /// [reth_primitives::TransactionSignedEcRecovered].
    #[error("failed to convert deposit transaction to TransactionSignedEcRecovered")]
    TransactionEcRecoverFailed,
    /// Thrown when the L1 block info could not be parsed from the calldata of the
    /// first transaction supplied in the payload attributes.
    #[error("failed to parse L1 block info from L1 info tx calldata")]
    L1BlockInfoParseFailed,
    /// Thrown when a database account could not be loaded.
    #[error("failed to load account {0:?}")]
    AccountLoadFailed(revm_primitives::Address),
    /// Thrown when force deploy of create2deployer code fails.
    #[error("failed to force create2deployer account code")]
    ForceCreate2DeployerFail,
}

impl From<oneshot::error::RecvError> for PayloadBuilderError {
    fn from(_: oneshot::error::RecvError) -> Self {
        PayloadBuilderError::ChannelClosed
    }
}
