//! Transaction validation abstractions.

/// Result returned after checking a transaction's validity
type TransactionValidationResult<Transaction> = Result<(), TransactionValidationError<Transaction>>;

/// Provides support for validating transaction at any given state of the chain
#[async_trait::async_trait]
pub trait TransactionValidator {
    /// The transaction type to validate
    type Transaction: Send + Sync;

    /// Validates the transaction and returns a validated outcome
    ///
    /// This is used by the transaction-pool check the transaction's validity against the state of
    /// the given block hash.
    async fn validate_transaction(
        &self,
        transaction: Self::Transaction,
        block_hash: (),
    ) -> TransactionValidationResult<Self::Transaction> {
        unimplemented!()
    }
}

/// Errors thrown during validity checks of a transaction
#[derive(Clone, PartialEq, Eq)]
pub enum TransactionValidationError<Transaction> {
    /// The transaction is considered invalid.
    ///
    /// Note: This does not indicate whether the transaction will not be valid in the future
    Invalid(Transaction),
    // TODO need variants for `Never`, or `At`?
}
