//! Transaction validation abstractions.

use crate::{
    error::InvalidPoolTransactionError,
    identifier::{SenderId, TransactionId},
    traits::{PoolTransaction, TransactionOrigin},
    MAX_INIT_CODE_SIZE, TX_MAX_SIZE,
};
use reth_primitives::{
    constants::ETHEREUM_BLOCK_GAS_LIMIT, Address, ChainSpec, IntoRecoveredTransaction,
    InvalidTransactionError, TransactionKind, TransactionSignedEcRecovered, TxHash,
    EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, LEGACY_TX_TYPE_ID, U256,
};
use reth_provider::{AccountReader, StateProviderFactory};
use std::{fmt, marker::PhantomData, sync::Arc, time::Instant};

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
    Invalid(T, InvalidPoolTransactionError),
    /// An error occurred while trying to validate the transaction
    Error(T, Box<dyn std::error::Error + Send + Sync>),
}

impl<T: PoolTransaction> TransactionValidationOutcome<T> {
    /// Returns the transaction that was validated.
    pub fn transaction(&self) -> &T {
        match self {
            Self::Valid { transaction, .. } => transaction,
            Self::Invalid(transaction, ..) => transaction,
            Self::Error(transaction, ..) => transaction,
        }
    }

    /// Returns the hash of the transactions
    pub fn tx_hash(&self) -> TxHash {
        *self.transaction().hash()
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
    /// See [InvalidTransactionError](InvalidTransactionError) for common errors variants.
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

/// A [TransactionValidator] implementation that validates ethereum transaction.
#[derive(Debug, Clone)]
pub struct EthTransactionValidator<Client, T> {
    /// Spec of the chain
    chain_spec: Arc<ChainSpec>,
    /// This type fetches account info from the db
    client: Client,
    /// Fork indicator whether we are in the Shanghai stage.
    shanghai: bool,
    /// Fork indicator whether we are using EIP-2718 type transactions.
    eip2718: bool,
    /// Fork indicator whether we are using EIP-1559 type transactions.
    eip1559: bool,
    /// The current max gas limit
    block_gas_limit: u64,
    /// Minimum priority fee to enforce for acceptance into the pool.
    minimum_priority_fee: Option<u128>,
    /// Marker for the transaction type
    _marker: PhantomData<T>,
}

// === impl EthTransactionValidator ===

impl<Client, Tx> EthTransactionValidator<Client, Tx> {
    /// Creates a new instance for the given [ChainSpec]
    pub fn new(client: Client, chain_spec: Arc<ChainSpec>) -> Self {
        // TODO(mattsse): improve these settings by checking against hardfork
        // See [reth_consensus::validation::validate_transaction_regarding_header]
        Self {
            chain_spec,
            client,
            shanghai: true,
            eip2718: true,
            eip1559: true,
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            minimum_priority_fee: None,
            _marker: Default::default(),
        }
    }

    /// Returns the configured chain id
    pub fn chain_id(&self) -> u64 {
        self.chain_spec.chain().id()
    }
}

#[async_trait::async_trait]
impl<Client, Tx> TransactionValidator for EthTransactionValidator<Client, Tx>
where
    Client: StateProviderFactory,
    Tx: PoolTransaction,
{
    type Transaction = Tx;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        // Checks for tx_type
        match transaction.tx_type() {
            LEGACY_TX_TYPE_ID => {
                // Accept legacy transactions
            }
            EIP2930_TX_TYPE_ID => {
                // Accept only legacy transactions until EIP-2718/2930 activates
                if !self.eip2718 {
                    return TransactionValidationOutcome::Invalid(
                        transaction,
                        InvalidTransactionError::Eip1559Disabled.into(),
                    )
                }
            }

            EIP1559_TX_TYPE_ID => {
                // Reject dynamic fee transactions until EIP-1559 activates.
                if !self.eip1559 {
                    return TransactionValidationOutcome::Invalid(
                        transaction,
                        InvalidTransactionError::Eip1559Disabled.into(),
                    )
                }
            }

            _ => {
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidTransactionError::TxTypeNotSupported.into(),
                )
            }
        };

        // Reject transactions over defined size to prevent DOS attacks
        if transaction.size() > TX_MAX_SIZE {
            let size = transaction.size();
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidPoolTransactionError::OversizedData(size, TX_MAX_SIZE),
            )
        }

        // Check whether the init code size has been exceeded.
        if self.shanghai {
            if let Err(err) = self.ensure_max_init_code_size(&transaction, MAX_INIT_CODE_SIZE) {
                return TransactionValidationOutcome::Invalid(transaction, err)
            }
        }

        // Checks for gas limit
        if transaction.gas_limit() > self.block_gas_limit {
            let gas_limit = transaction.gas_limit();
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidPoolTransactionError::ExceedsGasLimit(gas_limit, self.block_gas_limit),
            )
        }

        // Ensure max_priority_fee_per_gas (if EIP1559) is less than max_fee_per_gas if any.
        if transaction.max_priority_fee_per_gas() > Some(transaction.max_fee_per_gas()) {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::TipAboveFeeCap.into(),
            )
        }

        // Drop non-local transactions with a fee lower than the configured fee for acceptance into
        // the pool.
        if !origin.is_local() &&
            transaction.is_eip1559() &&
            transaction.max_priority_fee_per_gas() < self.minimum_priority_fee
        {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidPoolTransactionError::Underpriced,
            )
        }

        // Checks for chainid
        if let Some(chain_id) = transaction.chain_id() {
            if chain_id != self.chain_id() {
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidTransactionError::ChainIdMismatch.into(),
                )
            }
        }

        let account = match self
            .client
            .latest()
            .and_then(|state| state.basic_account(transaction.sender()))
        {
            Ok(account) => account.unwrap_or_default(),
            Err(err) => return TransactionValidationOutcome::Error(transaction, Box::new(err)),
        };

        // Signer account shouldn't have bytecode. Presence of bytecode means this is a
        // smartcontract.
        if account.has_bytecode() {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::SignerAccountHasBytecode.into(),
            )
        }

        // Checks for nonce
        if transaction.nonce() < account.nonce {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::NonceNotConsistent.into(),
            )
        }

        // Checks for max cost
        if transaction.cost() > account.balance {
            let cost = transaction.cost();
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::InsufficientFunds {
                    cost,
                    available_funds: account.balance,
                }
                .into(),
            )
        }

        // Return the valid transaction
        TransactionValidationOutcome::Valid {
            balance: account.balance,
            state_nonce: account.nonce,
            transaction,
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
    /// Total cost of the transaction: `feeCap x gasLimit + transferredValue`.
    pub cost: U256,
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

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    ///
    ///  For legacy transactions this is gas_price.
    pub fn max_fee_per_gas(&self) -> u128 {
        self.transaction.max_fee_per_gas()
    }

    /// Returns the EIP-1559 Max base fee the caller is willing to pay.
    pub fn effective_gas_price(&self) -> u128 {
        self.transaction.effective_gas_price()
    }

    /// Amount of gas that should be used in executing this transaction. This is paid up-front.
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
            cost: self.cost,
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
