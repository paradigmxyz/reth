//! Transaction validation abstractions.

use crate::{
    error::PoolError,
    identifier::{SenderId, TransactionId},
    traits::PoolTransaction,
};
use reth_primitives::{BlockID, TxHash, U256};
use std::fmt;

/// A Result type returned after checking a transaction's validity.
pub enum TransactionValidationOutcome<T: PoolTransaction> {
    /// Transaction successfully validated
    Valid {
        /// Balance of the sender at the current point.
        balance: U256,
        /// current nonce of the sender
        state_nonce: u64,
        /// Validated transaction.
        transaction: T,
    },
    /// The transaction is considered invalid.
    ///
    /// Note: This does not indicate whether the transaction will not be valid in the future
    Invalid(T, PoolError),
}

/// Provides support for validating transaction at any given state of the chain
#[async_trait::async_trait]
pub trait TransactionValidator: Send + Sync {
    /// The transaction type to validate.
    type Transaction: PoolTransaction + Send + Sync + 'static;

    /// Validates the transaction and returns a validated outcome
    ///
    /// This is used by the transaction-pool check the transaction's validity against the state of
    /// the given block hash.
    ///
    /// This is supposed to extend the `transaction` with its id in the graph of
    /// transactions for the sender.
    async fn validate_transaction(
        &self,
        _block_id: &BlockID,
        _transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        unimplemented!()
    }
}

/// A valida transaction in the pool.
pub struct ValidPoolTransaction<T: PoolTransaction> {
    /// The transaction
    pub transaction: T,
    /// Ids required by the transaction.
    ///
    /// This lists all unique transactions that need to be mined before this transaction can be
    /// considered `pending` and itself be included.
    pub depends_on: Vec<TransactionId>,
    /// The identifier for this transaction.
    pub transaction_id: TransactionId,
    /// Whether to propagate the transaction.
    pub propagate: bool,
    /// Internal `Sender` identifier
    pub sender_id: SenderId,
    /// Total cost of the transaction: `feeCap x gasLimit + transferred_value`
    pub cost: U256,
    // TODO add a block timestamp that marks validity
}

// === impl ValidPoolTransaction ===

impl<T: PoolTransaction> ValidPoolTransaction<T> {
    /// Returns the hash of the transaction
    pub fn hash(&self) -> &TxHash {
        self.transaction.hash()
    }
}

impl<T: PoolTransaction> fmt::Debug for ValidPoolTransaction<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "Transaction {{ ")?;
        write!(fmt, "hash: {:?}, ", &self.transaction.hash())?;
        write!(fmt, "provides: {:?}, ", &self.transaction_id)?;
        write!(fmt, "depends_on: {:?}, ", &self.depends_on)?;
        write!(fmt, "raw tx: {:?}", &self.transaction)?;
        write!(fmt, "}}")?;
        Ok(())
    }
}
