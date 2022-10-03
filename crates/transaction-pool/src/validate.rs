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
    ///
    /// This is supposed to extend the `transaction` with its identifying markers in the graph of
    /// transactions for the sender.
    async fn validate_transaction(
        &self,
        transaction: Self::Transaction,
        block_hash: (),
    ) -> TransactionValidationResult<ValidPoolTransaction<Self::Transaction>> {
        unimplemented!()
    }
}

/// Errors thrown during validity checks of a transaction.
#[derive(Clone, PartialEq, Eq)]
pub enum TransactionValidationError<Transaction> {
    /// The transaction is considered invalid.
    ///
    /// Note: This does not indicate whether the transaction will not be valid in the future
    Invalid(Transaction),
    // TODO need variants for `Never`, or `At`?
}

/// A valida transaction in the pool.
pub struct ValidPoolTransaction<T: PoolTransaction> {
    /// The transaction
    pub transaction: T,
    /// Ids required by the transaction.
    ///
    /// This lists all unique transactions that need to be mined before this transaction can be
    /// considered `pending` and itself be included.
    pub depends_on: Vec<T::Id>,
    /// Ids that this transaction provides
    ///
    /// This contains the inverse of `depends_on` which provides the dependencies this transaction
    /// unlocks once it's mined.
    pub provides: Vec<T::Id>,
}

impl<T: PoolTransaction> fmt::Debug for ValidPoolTransaction<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "Transaction {{ ")?;
        write!(fmt, "hash: {:?}, ", &self.transaction.hash())?;
        write!(fmt, "provides: {:?}, ", &self.provides)?;
        write!(fmt, "depends_on: {:?}, ", &self.depends_on)?;
        write!(fmt, "raw tx: {:?}", &self.transaction)?;
        write!(fmt, "}}")?;
        Ok(())
    }
}
