//! Error type

/// Optimism specific payload building errors.
#[derive(Debug, thiserror::Error)]
pub enum OptimismPayloadBuilderError {
    /// Thrown when a transaction fails to convert to a
    /// [`reth_primitives::TransactionSignedEcRecovered`].
    #[error("failed to convert deposit transaction to TransactionSignedEcRecovered")]
    TransactionEcRecoverFailed,
    /// Thrown when the L1 block info could not be parsed from the calldata of the
    /// first transaction supplied in the payload attributes.
    #[error("failed to parse L1 block info from L1 info tx calldata")]
    L1BlockInfoParseFailed,
    /// Thrown when a database account could not be loaded.
    #[error("failed to load account {0}")]
    AccountLoadFailed(alloy_primitives::Address),
    /// Thrown when force deploy of create2deployer code fails.
    #[error("failed to force create2deployer account code")]
    ForceCreate2DeployerFail,
    /// Thrown when a blob transaction is included in a sequencer's block.
    #[error("blob transaction included in sequencer block")]
    BlobTransactionRejected,
}

/// Error type for EIP-1559 parameters
#[derive(Debug, thiserror::Error)]
pub enum EIP1559ParamError {
    /// No EIP-1559 parameters provided
    #[error("No EIP-1559 parameters provided")]
    NoEIP1559Params,
    /// Denominator overflow
    #[error("Denominator overflow")]
    DenominatorOverflow,
    /// Elasticity overflow
    #[error("Elasticity overflow")]
    ElasticityOverflow,
}
