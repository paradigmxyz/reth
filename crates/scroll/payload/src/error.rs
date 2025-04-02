/// Scroll specific payload building errors.
#[derive(Debug, thiserror::Error)]
pub enum ScrollPayloadBuilderError {
    /// Thrown when a transaction fails to convert to a
    /// [`alloy_consensus::transaction::Recovered`].
    #[error("failed to convert deposit transaction to RecoveredTx")]
    TransactionEcRecoverFailed,
    /// Thrown when a blob transaction is included in a sequencer's block.
    #[error("blob transaction included in sequencer block")]
    BlobTransactionRejected,
}
