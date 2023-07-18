//! Transaction validation abstractions.

use crate::{
    error::InvalidPoolTransactionError,
    identifier::{SenderId, TransactionId},
    traits::{PoolTransaction, TransactionOrigin},
};
use reth_primitives::{
    Address, IntoRecoveredTransaction, TransactionKind, TransactionSignedEcRecovered, TxHash, U256,
};
use std::{fmt, time::Instant};

mod eth;
mod task;

/// A [TransactionValidator] implementation that validates ethereum transaction.
pub use eth::{EthTransactionValidator, EthTransactionValidatorBuilder};

/// A spawnable task that performs transaction validation.
pub use task::ValidationTask;

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
        /// Whether to propagate the transaction to the network.
        propagate: bool,
    },
    /// The transaction is considered invalid indefinitely: It violates constraints that prevent
    /// this transaction from ever becoming valid.
    Invalid(T, InvalidPoolTransactionError),
    /// An error occurred while trying to validate the transaction
    Error(TxHash, Box<dyn std::error::Error + Send + Sync>),
}

impl<T: PoolTransaction> TransactionValidationOutcome<T> {
    /// Returns the hash of the transactions
    pub fn tx_hash(&self) -> TxHash {
        match self {
            Self::Valid { transaction, .. } => *transaction.hash(),
            Self::Invalid(transaction, ..) => *transaction.hash(),
            Self::Error(hash, ..) => *hash,
        }
    }
}

/// Provides support for validating transaction at any given state of the chain
#[async_trait::async_trait]
pub trait TransactionValidator: Send + Sync {
    /// The transaction type to validate.
    type Transaction: PoolTransaction;

    /// Validates the transaction and returns a [`TransactionValidationOutcome`] describing the
    /// validity of the given transaction.
    ///
    /// This will be used by the transaction-pool to check whether the transaction should be
    /// inserted into the pool or discarded right away.
    ///
    /// Implementers of this trait must ensure that the transaction is well-formed, i.e. that it
    /// complies at least all static constraints, which includes checking for:
    ///
    ///    * chain id
    ///    * gas limit
    ///    * max cost
    ///    * nonce >= next nonce of the sender
    ///    * ...
    ///
    /// See [InvalidTransactionError](reth_primitives::InvalidTransactionError) for common errors
    /// variants.
    ///
    /// The transaction pool makes no additional assumptions about the validity of the transaction
    /// at the time of this call before it inserts it into the pool. However, the validity of
    /// this transaction is still subject to future (dynamic) changes enforced by the pool, for
    /// example nonce or balance changes. Hence, any validation checks must be applied in this
    /// function.
    ///
    /// See [EthTransactionValidator] for a reference implementation.
    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction>;

    /// Ensure that the code size is not greater than `max_init_code_size`.
    /// `max_init_code_size` should be configurable so this will take it as an argument.
    fn ensure_max_init_code_size(
        &self,
        transaction: &Self::Transaction,
        max_init_code_size: usize,
    ) -> Result<(), InvalidPoolTransactionError> {
        if *transaction.kind() == TransactionKind::Create && transaction.size() > max_init_code_size
        {
            Err(InvalidPoolTransactionError::ExceedsMaxInitCodeSize(
                transaction.size(),
                max_init_code_size,
            ))
        } else {
            Ok(())
        }
    }
}

/// A valid transaction in the pool.
pub struct ValidPoolTransaction<T: PoolTransaction> {
    /// The transaction
    pub transaction: T,
    /// The identifier for this transaction.
    pub transaction_id: TransactionId,
    /// Whether to propagate the transaction.
    pub propagate: bool,
    /// Timestamp when this was added to the pool.
    pub timestamp: Instant,
    /// Where this transaction originated from.
    pub origin: TransactionOrigin,
    /// The length of the rlp encoded transaction (cached)
    pub encoded_length: usize,
}

// === impl ValidPoolTransaction ===

impl<T: PoolTransaction> ValidPoolTransaction<T> {
    /// Returns the hash of the transaction.
    pub fn hash(&self) -> &TxHash {
        self.transaction.hash()
    }

    /// Returns the type identifier of the transaction
    pub fn tx_type(&self) -> u8 {
        self.transaction.tx_type()
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

    /// Returns the cost that this transaction is allowed to consume:
    ///
    /// For EIP-1559 transactions: `max_fee_per_gas * gas_limit + tx_value`.
    /// For legacy transactions: `gas_price * gas_limit + tx_value`.
    pub fn cost(&self) -> U256 {
        self.transaction.cost()
    }

    /// Returns the gas cost for this transaction.
    ///
    /// For EIP-1559 transactions: `max_fee_per_gas * gas_limit`.
    /// For legacy transactions: `gas_price * gas_limit`.
    pub fn gas_cost(&self) -> U256 {
        self.transaction.gas_cost()
    }

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    /// For legacy transactions this is `gas_price`.
    pub fn max_fee_per_gas(&self) -> u128 {
        self.transaction.max_fee_per_gas()
    }

    /// Maximum amount of gas that the transaction is allowed to consume.
    pub fn gas_limit(&self) -> u64 {
        self.transaction.gas_limit()
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

impl<T: PoolTransaction> IntoRecoveredTransaction for ValidPoolTransaction<T> {
    fn to_recovered_transaction(&self) -> TransactionSignedEcRecovered {
        self.transaction.to_recovered_transaction()
    }
}

#[cfg(test)]
impl<T: PoolTransaction + Clone> Clone for ValidPoolTransaction<T> {
    fn clone(&self) -> Self {
        Self {
            transaction: self.transaction.clone(),
            transaction_id: self.transaction_id,
            propagate: self.propagate,
            timestamp: self.timestamp,
            origin: self.origin,
            encoded_length: self.encoded_length,
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

/// Validation Errors that can occur during transaction validation.
#[derive(thiserror::Error, Debug)]
pub enum TransactionValidatorError {
    /// Failed to communicate with the validation service.
    #[error("Validation service unreachable")]
    ValidationServiceUnreachable,
}
