//! Ethereum transaction validator.

use crate::{
    error::InvalidPoolTransactionError,
    traits::{PoolTransaction, TransactionOrigin},
    validate::{task::ValidationJobSender, TransactionValidatorError, ValidationTask},
    TransactionValidationOutcome, TransactionValidator, MAX_INIT_CODE_SIZE, TX_MAX_SIZE,
};
use reth_primitives::{
    constants::ETHEREUM_BLOCK_GAS_LIMIT, ChainSpec, InvalidTransactionError, EIP1559_TX_TYPE_ID,
    EIP2930_TX_TYPE_ID, LEGACY_TX_TYPE_ID,
};
use reth_provider::{AccountReader, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::{marker::PhantomData, sync::Arc};
use tokio::sync::{oneshot, Mutex};

/// A [TransactionValidator] implementation that validates ethereum transaction.
///
/// This validator is non-blocking, all validation work is done in a separate task.
#[derive(Debug, Clone)]
pub struct EthTransactionValidator<Client, T> {
    /// The type that performs the actual validation.
    inner: Arc<EthTransactionValidatorInner<Client, T>>,
    /// The sender half to validation tasks that perform the actual validation.
    to_validation_task: Arc<Mutex<ValidationJobSender>>,
}

// === impl EthTransactionValidator ===

impl<Client, Tx> EthTransactionValidator<Client, Tx> {
    /// Creates a new instance for the given [ChainSpec]
    ///
    /// This will always spawn a validation tasks that perform the actual validation. A will spawn
    /// `num_additional_tasks` additional tasks.
    pub fn new<T>(
        client: Client,
        chain_spec: Arc<ChainSpec>,
        tasks: T,
        num_additional_tasks: usize,
    ) -> Self
    where
        T: TaskSpawner,
    {
        let inner = EthTransactionValidatorInner {
            chain_spec,
            client,
            shanghai: true,
            eip2718: true,
            eip1559: true,
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            minimum_priority_fee: None,
            _marker: Default::default(),
        };

        let (tx, task) = ValidationTask::new();

        // Spawn validation tasks, they are blocking because they perform db lookups
        for _ in 0..num_additional_tasks {
            let task = task.clone();
            tasks.spawn_blocking(Box::pin(async move {
                task.run().await;
            }));
        }

        tasks.spawn_critical_blocking(
            "transaction-validation-service",
            Box::pin(async move {
                task.run().await;
            }),
        );

        let to_validation_task = Arc::new(Mutex::new(tx));

        Self { inner: Arc::new(inner), to_validation_task }
    }

    /// Returns the configured chain id
    pub fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }
}

#[async_trait::async_trait]
impl<Client, Tx> TransactionValidator for EthTransactionValidator<Client, Tx>
where
    Client: StateProviderFactory + Clone + 'static,
    Tx: PoolTransaction + 'static,
{
    type Transaction = Tx;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        let hash = *transaction.hash();
        let (tx, rx) = oneshot::channel();
        {
            let to_validation_task = self.to_validation_task.clone();
            let to_validation_task = to_validation_task.lock().await;
            let validator = Arc::clone(&self.inner);
            let res = to_validation_task
                .send(Box::pin(async move {
                    let res = validator.validate_transaction(origin, transaction).await;
                    let _ = tx.send(res);
                }))
                .await;
            if res.is_err() {
                return TransactionValidationOutcome::Error(
                    hash,
                    Box::new(TransactionValidatorError::ValidationServiceUnreachable),
                )
            }
        }

        match rx.await {
            Ok(res) => res,
            Err(_) => TransactionValidationOutcome::Error(
                hash,
                Box::new(TransactionValidatorError::ValidationServiceUnreachable),
            ),
        }
    }
}

/// A [TransactionValidator] implementation that validates ethereum transaction.
#[derive(Debug, Clone)]
struct EthTransactionValidatorInner<Client, T> {
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

// === impl EthTransactionValidatorInner ===

impl<Client, Tx> EthTransactionValidatorInner<Client, Tx> {
    /// Returns the configured chain id
    fn chain_id(&self) -> u64 {
        self.chain_spec.chain().id()
    }
}

#[async_trait::async_trait]
impl<Client, Tx> TransactionValidator for EthTransactionValidatorInner<Client, Tx>
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
            Err(err) => {
                return TransactionValidationOutcome::Error(*transaction.hash(), Box::new(err))
            }
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
        let cost = transaction.cost();
        if cost > account.balance {
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
