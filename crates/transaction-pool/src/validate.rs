//! Transaction validation abstractions.

use crate::traits::PoolTransaction;
use std::{fmt, hash::Hash};

/// Result returned after checking a transaction's validity
type TransactionValidationResult<Transaction> = Result<(), TransactionValidationError<Transaction>>;

/// Provides support for validating transaction at any given state of the chain
#[async_trait::async_trait]
pub trait TransactionValidator: Send + Sync {
    /// The transaction type to validate
    type Transaction: PoolTransaction + Send + Sync;

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

/// Already validated transaction that can be queued.
pub trait ValidatedTransaction: fmt::Debug {
    /// Transaction hash type.
    type Hash: fmt::Debug + fmt::LowerHex + Eq + Clone + Hash;

    /// Transaction sender type.
    type Sender: fmt::Debug + Eq + Clone + Hash + Send;

    /// Transaction hash
    fn hash(&self) -> &Self::Hash;

    /// Memory usage of this transaction
    fn size(&self) -> usize;

    /// Transaction sender
    fn sender(&self) -> &Self::Sender;

    /// Does it have zero gas price?
    fn has_zero_gas_price(&self) -> bool;
}
