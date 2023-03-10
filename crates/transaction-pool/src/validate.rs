//! Transaction validation abstractions.

use crate::{
    error::PoolError,
    identifier::{SenderId, TransactionId},
    traits::{PoolTransaction, TransactionOrigin},
    MAX_INIT_CODE_SIZE, TX_MAX_SIZE,
};
use reth_interfaces::consensus::Error;
use reth_primitives::{
    Address, TransactionKind, TxHash, EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, LEGACY_TX_TYPE_ID,
    U256,
};
use reth_provider::AccountProvider;
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
pub trait TransactionValidator: Send + Sync {
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
    ///    * chain id
    ///    * gas limit
    ///    * max cost
    ///    * nonce >= next nonce of the sender
    ///
    /// The transaction pool makes no assumptions about the validity of the transaction at the time
    /// of this call before it inserts it. However, the validity of this transaction is still
    /// subject to future changes enforced by the pool, for example nonce changes.
    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> Result<TransactionValidationOutcome<Self::Transaction>, Error>;

    /// Ensure that the code size is not greater than `max_init_code_size`.
    /// `max_init_code_size` should be configurable so this will take it as an argument.
    fn ensure_max_init_code_size(
        &self,
        transaction: Self::Transaction,
        max_init_code_size: usize,
    ) -> Result<(), PoolError> {
        if *transaction.kind() == TransactionKind::Create && transaction.size() > max_init_code_size
        {
            Err(PoolError::TxExceedsMaxInitCodeSize(
                *transaction.hash(),
                transaction.size(),
                max_init_code_size,
            ))
        } else {
            Ok(())
        }
    }
}

/// TODO: Add docs and make this public
pub(crate) struct EthTransactionValidatorConfig<Client: AccountProvider> {
    /// Chain id
    chain_id: u64,
    /// This type fetches account info from the db
    client: Client,
    /// Fork indicator whether we are in the Shanghai stage.
    shanghai: bool,
    /// Fork indicator whether we are using EIP-2718 type transactions.
    eip2718: bool,
    /// Fork indicator whether we are using EIP-1559 type transactions.
    eip1559: bool,
    /// The current max gas limit
    current_max_gas_limit: u64,
    /// gasprice
    gas_price: Option<u128>,
}

#[async_trait::async_trait]
impl<T: PoolTransaction + AccountProvider + Clone> TransactionValidator
    for EthTransactionValidatorConfig<T>
{
    type Transaction = T;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> Result<TransactionValidationOutcome<Self::Transaction>, Error> {
        // Checks for tx_type
        match transaction.tx_type() {
            LEGACY_TX_TYPE_ID => {
                // Accept legacy transactions
            }

            EIP2930_TX_TYPE_ID => {
                // Accept only legacy transactions until EIP-2718/2930 activates
                if !self.eip2718 {
                    return Err(Error::TransactionEip2930Disabled)
                }
            }

            EIP1559_TX_TYPE_ID => {
                // Reject dynamic fee transactions until EIP-1559 activates.
                if !self.eip1559 {
                    return Err(Error::TransactionEip1559Disabled)
                }
            }

            _ => return Err(Error::TxTypeNotSupported),
        };

        // Reject transactions over defined size to prevent DOS attacks
        if transaction.size() > TX_MAX_SIZE {
            return Ok(TransactionValidationOutcome::Invalid(
                transaction.clone(),
                PoolError::OversizedData(*transaction.hash(), transaction.size(), TX_MAX_SIZE),
            ))
        }

        // Check whether the init code size has been exceeded.
        if self.shanghai {
            match self.ensure_max_init_code_size(transaction.clone(), MAX_INIT_CODE_SIZE) {
                Ok(_) => {}
                Err(e) => return Ok(TransactionValidationOutcome::Invalid(transaction, e)),
            }
        }

        // Checks for gas limit
        if transaction.gas_limit() > self.current_max_gas_limit {
            return Ok(TransactionValidationOutcome::Invalid(
                transaction.clone(),
                PoolError::TxExceedsGasLimit(
                    *transaction.hash(),
                    transaction.gas_limit(),
                    self.current_max_gas_limit,
                ),
            ))
        }

        // Ensure max_fee_per_gas is greater than or equal to max_priority_fee_per_gas.
        if transaction.max_fee_per_gas() <= transaction.max_priority_fee_per_gas() {
            return Err(Error::TipAboveFeeCap)
        }

        // Drop non-local transactions under our own minimal accepted gas price or tip
        if !origin.is_local() && transaction.max_fee_per_gas() < self.gas_price {
            return Err(Error::TransactionMaxFeeLessThenBaseFee)
        }

        // Checks for chainid
        if transaction.chain_id() != Some(self.chain_id) {
            return Err(Error::TransactionChainId)
        }

        let account = match self.client.basic_account(transaction.sender())? {
            Some(account) => {
                // Signer account shouldn't have bytecode. Presence of bytecode means this is a
                // smartcontract.
                if account.has_bytecode() {
                    return Err(Error::SignerAccountHasBytecode)
                } else {
                    account
                }
            }
            None => {
                return Ok(TransactionValidationOutcome::Invalid(
                    transaction.clone(),
                    PoolError::AccountNotFound(*transaction.hash()),
                ))
            }
        };

        // Checks for nonce
        if transaction.nonce() < account.nonce {
            return Err(Error::TransactionNonceNotConsistent)
        }

        // Checks for max cost
        if transaction.cost() > account.balance {
            return Err(Error::InsufficientFunds {
                max_fee: transaction.max_fee_per_gas().unwrap_or_default(),
                available_funds: account.balance,
            })
        }

        // Return the valid transaction
        Ok(TransactionValidationOutcome::Valid {
            balance: account.balance,
            state_nonce: account.nonce,
            transaction,
        })
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
    pub fn max_fee_per_gas(&self) -> Option<u128> {
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
