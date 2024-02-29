//! Transaction validation abstractions.

use crate::{
    error::InvalidPoolTransactionError,
    identifier::{SenderId, TransactionId},
    traits::{PoolTransaction, TransactionOrigin},
};
use reth_primitives::{
    Address, BlobTransactionSidecar, IntoRecoveredTransaction, SealedBlock,
    TransactionSignedEcRecovered, TxHash, B256, U256,
};
use std::{fmt, future::Future, time::Instant};

mod constants;
mod eth;
mod task;

/// A `TransactionValidator` implementation that validates ethereum transaction.
pub use eth::*;

/// A spawnable task that performs transaction validation.
pub use task::{TransactionValidationTaskExecutor, ValidationTask};

/// Validation constants.
pub use constants::{
    DEFAULT_MAX_TX_INPUT_BYTES, MAX_CODE_BYTE_SIZE, MAX_INIT_CODE_BYTE_SIZE, TX_SLOT_BYTE_SIZE,
};

/// A Result type returned after checking a transaction's validity.
#[derive(Debug)]
pub enum TransactionValidationOutcome<T: PoolTransaction> {
    /// The transaction is considered _currently_ valid and can be inserted into the pool.
    Valid {
        /// Balance of the sender at the current point.
        balance: U256,
        /// Current nonce of the sender.
        state_nonce: u64,
        /// The validated transaction.
        ///
        /// See also [ValidTransaction].
        ///
        /// If this is a _new_ EIP-4844 blob transaction, then this must contain the extracted
        /// sidecar.
        transaction: ValidTransaction<T>,
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

    /// Returns true if the transaction is valid.
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }

    /// Returns true if the transaction is invalid.
    pub const fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid(_, _))
    }

    /// Returns true if validation resulted in an error.
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Error(_, _))
    }
}

/// A wrapper type for a transaction that is valid and has an optional extracted EIP-4844 blob
/// transaction sidecar.
///
/// If this is provided, then the sidecar will be temporarily stored in the blob store until the
/// transaction is finalized.
///
/// Note: Since blob transactions can be re-injected without their sidecar (after reorg), the
/// validator can omit the sidecar if it is still in the blob store and return a
/// [ValidTransaction::Valid] instead.
#[derive(Debug)]
pub enum ValidTransaction<T> {
    /// A valid transaction without a sidecar.
    Valid(T),
    /// A valid transaction for which a sidecar should be stored.
    ///
    /// Caution: The [TransactionValidator] must ensure that this is only returned for EIP-4844
    /// transactions.
    ValidWithSidecar {
        /// The valid EIP-4844 transaction.
        transaction: T,
        /// The extracted sidecar of that transaction
        sidecar: BlobTransactionSidecar,
    },
}

impl<T> ValidTransaction<T> {
    /// Creates a new valid transaction with an optional sidecar.
    pub fn new(transaction: T, sidecar: Option<BlobTransactionSidecar>) -> Self {
        if let Some(sidecar) = sidecar {
            Self::ValidWithSidecar { transaction, sidecar }
        } else {
            Self::Valid(transaction)
        }
    }
}

impl<T: PoolTransaction> ValidTransaction<T> {
    #[inline]
    pub(crate) const fn transaction(&self) -> &T {
        match self {
            Self::Valid(transaction) | Self::ValidWithSidecar { transaction, .. } => transaction,
        }
    }

    /// Returns the address of that transaction.
    #[inline]
    pub(crate) fn sender(&self) -> Address {
        self.transaction().sender()
    }

    /// Returns the hash of the transaction.
    #[inline]
    pub(crate) fn hash(&self) -> &B256 {
        self.transaction().hash()
    }

    /// Returns the nonce of the transaction.
    #[inline]
    pub(crate) fn nonce(&self) -> u64 {
        self.transaction().nonce()
    }
}

/// Provides support for validating transaction at any given state of the chain
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
    /// See [TransactionValidationTaskExecutor] for a reference implementation.
    fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> impl Future<Output = TransactionValidationOutcome<Self::Transaction>> + Send;

    /// Validates a batch of transactions.
    ///
    /// Must return all outcomes for the given transactions in the same order.
    ///
    /// See also [Self::validate_transaction].
    fn validate_transactions(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> impl Future<Output = Vec<TransactionValidationOutcome<Self::Transaction>>> + Send {
        async {
            futures_util::future::join_all(
                transactions.into_iter().map(|(origin, tx)| self.validate_transaction(origin, tx)),
            )
            .await
        }
    }

    /// Invoked when the head block changes.
    ///
    /// This can be used to update fork specific values (timestamp).
    fn on_new_head_block(&self, _new_tip_block: &SealedBlock) {}
}

/// A valid transaction in the pool.
///
/// This is used as the internal representation of a transaction inside the pool.
///
/// For EIP-4844 blob transactions this will _not_ contain the blob sidecar which is stored
/// separately in the [BlobStore](crate::blobstore::BlobStore).
pub struct ValidPoolTransaction<T: PoolTransaction> {
    /// The transaction
    pub transaction: T,
    /// The identifier for this transaction.
    pub transaction_id: TransactionId,
    /// Whether it is allowed to propagate the transaction.
    pub propagate: bool,
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

    /// Returns the type identifier of the transaction
    pub fn tx_type(&self) -> u8 {
        self.transaction.tx_type()
    }

    /// Returns the address of the sender
    pub fn sender(&self) -> Address {
        self.transaction.sender()
    }

    /// Returns the recipient of the transaction if it is not a CREATE transaction.
    pub fn to(&self) -> Option<Address> {
        self.transaction.to()
    }

    /// Returns the internal identifier for the sender of this transaction
    pub(crate) const fn sender_id(&self) -> SenderId {
        self.transaction_id.sender
    }

    /// Returns the internal identifier for this transaction.
    pub(crate) const fn id(&self) -> &TransactionId {
        &self.transaction_id
    }

    /// Returns the length of the rlp encoded transaction
    #[inline]
    pub fn encoded_length(&self) -> usize {
        self.transaction.encoded_length()
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

    /// Returns the EIP-4844 max blob fee the caller is willing to pay.
    ///
    /// For non-EIP-4844 transactions, this returns [None].
    pub fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.transaction.max_fee_per_blob_gas()
    }

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    /// For legacy transactions this is `gas_price`.
    pub fn max_fee_per_gas(&self) -> u128 {
        self.transaction.max_fee_per_gas()
    }

    /// Returns the effective tip for this transaction.
    ///
    /// For EIP-1559 transactions: `min(max_fee_per_gas - base_fee, max_priority_fee_per_gas)`.
    /// For legacy transactions: `gas_price - base_fee`.
    pub fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        self.transaction.effective_tip_per_gas(base_fee)
    }

    /// Returns the max priority fee per gas if the transaction is an EIP-1559 transaction, and
    /// otherwise returns the gas price.
    pub fn priority_fee_or_price(&self) -> u128 {
        self.transaction.priority_fee_or_price()
    }

    /// Maximum amount of gas that the transaction is allowed to consume.
    pub fn gas_limit(&self) -> u64 {
        self.transaction.gas_limit()
    }

    /// Whether the transaction originated locally.
    pub const fn is_local(&self) -> bool {
        self.origin.is_local()
    }

    /// Whether the transaction is an EIP-4844 blob transaction.
    #[inline]
    pub fn is_eip4844(&self) -> bool {
        self.transaction.is_eip4844()
    }

    /// The heap allocated size of this transaction.
    pub(crate) fn size(&self) -> usize {
        self.transaction.size()
    }

    /// EIP-4844 blob transactions and normal transactions are treated as mutually exclusive per
    /// account.
    ///
    /// Returns true if the transaction is an EIP-4844 blob transaction and the other is not, or
    /// vice versa.
    #[inline]
    pub(crate) fn tx_type_conflicts_with(&self, other: &Self) -> bool {
        self.is_eip4844() != other.is_eip4844()
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
        }
    }
}

impl<T: PoolTransaction> fmt::Debug for ValidPoolTransaction<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidPoolTransaction")
            .field("hash", self.transaction.hash())
            .field("provides", &self.transaction_id)
            .field("raw_tx", &self.transaction)
            .finish()
    }
}

/// Validation Errors that can occur during transaction validation.
#[derive(thiserror::Error, Debug)]
pub enum TransactionValidatorError {
    /// Failed to communicate with the validation service.
    #[error("validation service unreachable")]
    ValidationServiceUnreachable,
}
