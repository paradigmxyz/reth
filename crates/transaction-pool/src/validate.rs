//! Transaction validation abstractions.

use crate::{
    error::PoolError,
    identifier::{SenderId, TransactionId},
    traits::{PoolTransaction, TransactionOrigin},
};
use reth_primitives::{rpc::Address, TxHash, U256};
use std::{fmt, time::Instant};

/// A Result type returned after checking a transaction's validity.
#[derive(Debug)]
pub enum TransactionValidationOutcome<T: PoolTransaction> {
    /// The transaction is considered _currently_ valid and can be inserted into the pool.
    Valid {
        /// Balance of the sender at the current point.
        balance: U256,
        /// Current nonce of the sender.
        state_nonce: u64,
        /// Validated transaction.
        transaction: T,
    },
    /// The transaction is considered invalid indefinitely: It violates constraints that prevent
    /// this transaction from ever becoming valid.
    Invalid(T, PoolError),
}

/// Provides support for validating transaction at any given state of the chain
#[async_trait::async_trait]
pub trait TransactionValidator: Send + Sync + 'static {
    /// The transaction type to validate.
    type Transaction: PoolTransaction;

    /// Validates the transaction and returns a [`TransactionValidationOutcome`] describing the
    /// validity of the given transaction.
    ///
    /// This will be used by the transaction-pool to check whether the transaction should be
    /// inserted into the pool or discarded right away.
    ///
    ///
    /// Implementers of this trait must ensure that the transaction is correct, i.e. that it
    /// complies with at least all static constraints, which includes checking for:
    ///
    ///     * chain id
    ///     * gas limit
    ///     * max cost
    ///     * nonce >= next nonce of the sender
    ///
    /// The transaction pool makes no assumptions about the validity of the transaction at the time
    /// of this call before it inserts it. However, the validity of this transaction is still
    /// subject to future changes enforced by the pool, for example nonce changes.
    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction>;
}

/// A valida transaction in the pool.
pub struct ValidPoolTransaction<T: PoolTransaction> {
    /// The transaction
    pub transaction: T,
    /// The identifier for this transaction.
    pub transaction_id: TransactionId,
    /// Whether to propagate the transaction.
    pub propagate: bool,
    /// Total cost of the transaction: `feeCap x gasLimit + transferred_value`.
    pub cost: U256,
    /// Timestamp when this was added to the pool.
    pub timestamp: Instant,
    /// Where this transaction originated from.
    pub origin: TransactionOrigin,
}

// === impl ValidPoolTransaction ===

impl<T: PoolTransaction> ValidPoolTransaction<T> {
    /// Returns the hash of the transaction.
    pub fn hash(&self) -> &TxHash {
        self.transaction.hash()
    }

    /// Returns the address of the sender
    pub fn sender(&self) -> Address {
        self.transaction.sender()
    }

    /// Returns the internal identifier for the sender of this transaction
    pub(crate) fn sender_id(&self) -> SenderId {
        self.transaction_id.sender
    }

    /// Returns the internal identifier for this transaction.
    pub(crate) fn id(&self) -> &TransactionId {
        &self.transaction_id
    }

    /// Returns the nonce set for this transaction.
    pub fn nonce(&self) -> u64 {
        self.transaction.nonce()
    }

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    pub fn max_fee_per_gas(&self) -> Option<U256> {
        self.transaction.max_fee_per_gas()
    }

    /// Amount of gas that should be used in executing this transaction. This is paid up-front.
    pub fn gas_limit(&self) -> u64 {
        self.transaction.gas_limit()
    }

    /// Returns true if this transaction is underpriced compared to the other.
    pub(crate) fn is_underpriced(&self, other: &Self) -> bool {
        self.transaction.effective_gas_price() <= other.transaction.effective_gas_price()
    }

    /// Whether the transaction originated locally.
    pub fn is_local(&self) -> bool {
        self.origin.is_local()
    }

    /// The heap allocated size of this transaction.
    pub(crate) fn size(&self) -> usize {
        self.transaction.size()
    }
}

#[cfg(test)]
impl<T: PoolTransaction + Clone> Clone for ValidPoolTransaction<T> {
    fn clone(&self) -> Self {
        Self {
            transaction: self.transaction.clone(),
            transaction_id: self.transaction_id,
            propagate: self.propagate,
            cost: self.cost,
            timestamp: self.timestamp,
            origin: self.origin,
        }
    }
}

impl<T: PoolTransaction> fmt::Debug for ValidPoolTransaction<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "Transaction {{ ")?;
        write!(fmt, "hash: {:?}, ", &self.transaction.hash())?;
        write!(fmt, "provides: {:?}, ", &self.transaction_id)?;
        write!(fmt, "raw tx: {:?}", &self.transaction)?;
        write!(fmt, "}}")?;
        Ok(())
    }
}
