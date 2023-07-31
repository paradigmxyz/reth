//! Ethereum transaction validator.

use crate::{
    error::InvalidPoolTransactionError,
    traits::{PoolTransaction, TransactionOrigin},
    validate::{
        task::ValidationJobSender, TransactionValidatorError, ValidationTask, MAX_INIT_CODE_SIZE,
        TX_MAX_SIZE,
    },
    TransactionValidationOutcome, TransactionValidator,
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

impl EthTransactionValidator<(), ()> {
    /// Convenience method to create a [EthTransactionValidatorBuilder]
    pub fn builder(chain_spec: Arc<ChainSpec>) -> EthTransactionValidatorBuilder {
        EthTransactionValidatorBuilder::new(chain_spec)
    }
}

impl<Client, Tx> EthTransactionValidator<Client, Tx> {
    /// Creates a new instance for the given [ChainSpec]
    ///
    /// This will spawn a single validation tasks that performs the actual validation.
    /// See [EthTransactionValidator::with_additional_tasks]
    pub fn new<T>(client: Client, chain_spec: Arc<ChainSpec>, tasks: T) -> Self
    where
        T: TaskSpawner,
    {
        Self::with_additional_tasks(client, chain_spec, tasks, 0)
    }

    /// Creates a new instance for the given [ChainSpec]
    ///
    /// By default this will enable support for:
    ///   - shanghai
    ///   - eip1559
    ///   - eip2930
    ///
    /// This will always spawn a validation task that performs the actual validation. It will spawn
    /// `num_additional_tasks` additional tasks.
    pub fn with_additional_tasks<T>(
        client: Client,
        chain_spec: Arc<ChainSpec>,
        tasks: T,
        num_additional_tasks: usize,
    ) -> Self
    where
        T: TaskSpawner,
    {
        EthTransactionValidatorBuilder::new(chain_spec)
            .with_additional_tasks(num_additional_tasks)
            .build(client, tasks)
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

/// A builder for [EthTransactionValidator]
#[derive(Debug, Clone)]
pub struct EthTransactionValidatorBuilder {
    chain_spec: Arc<ChainSpec>,
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
    /// Determines how many additional tasks to spawn
    ///
    /// Default is 1
    additional_tasks: usize,
    /// Toggle to determine if a local transaction should be propagated
    propagate_local_transactions: bool,
}

impl EthTransactionValidatorBuilder {
    /// Creates a new builder for the given [ChainSpec]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            chain_spec,
            shanghai: true,
            eip2718: true,
            eip1559: true,
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            minimum_priority_fee: None,
            additional_tasks: 1,
            // default to true, can potentially take this as a param in the future
            propagate_local_transactions: true,
        }
    }

    /// Disables the Shanghai fork.
    pub fn no_shanghai(self) -> Self {
        self.set_shanghai(false)
    }

    /// Set the Shanghai fork.
    pub fn set_shanghai(mut self, shanghai: bool) -> Self {
        self.shanghai = shanghai;
        self
    }

    /// Disables the eip2718 support.
    pub fn no_eip2718(self) -> Self {
        self.set_eip2718(false)
    }

    /// Set eip2718 support.
    pub fn set_eip2718(mut self, eip2718: bool) -> Self {
        self.eip2718 = eip2718;
        self
    }

    /// Disables the eip1559 support.
    pub fn no_eip1559(self) -> Self {
        self.set_eip1559(false)
    }

    /// Set the eip1559 support.
    pub fn set_eip1559(mut self, eip1559: bool) -> Self {
        self.eip1559 = eip1559;
        self
    }
    /// Sets toggle to propagate transactions received locally by this client (e.g
    /// transactions from eth_Sendtransaction to this nodes' RPC server)
    ///
    ///  If set to false, only transactions received by network peers (via
    /// p2p) will be marked as propagated in the local transaction pool and returned on a
    /// GetPooledTransactions p2p request
    pub fn set_propagate_local_transactions(mut self, propagate_local_txs: bool) -> Self {
        self.propagate_local_transactions = propagate_local_txs;
        self
    }
    /// Disables propagating transactions recieved locally by this client
    ///
    /// For more information, check docs for set_propagate_local_transactions
    pub fn no_local_transaction_propagation(mut self) -> Self {
        self.propagate_local_transactions = false;
        self
    }

    /// Sets a minimum priority fee that's enforced for acceptance into the pool.
    pub fn with_minimum_priority_fee(mut self, minimum_priority_fee: u128) -> Self {
        self.minimum_priority_fee = Some(minimum_priority_fee);
        self
    }

    /// Sets the number of additional tasks to spawn.
    pub fn with_additional_tasks(mut self, additional_tasks: usize) -> Self {
        self.additional_tasks = additional_tasks;
        self
    }

    /// Builds a [EthTransactionValidator]
    ///
    /// The validator will spawn `additional_tasks` additional tasks for validation.
    ///
    /// By default this will spawn 1 additional task.
    pub fn build<Client, Tx, T>(
        self,
        client: Client,
        tasks: T,
    ) -> EthTransactionValidator<Client, Tx>
    where
        T: TaskSpawner,
    {
        let Self {
            chain_spec,
            shanghai,
            eip2718,
            eip1559,
            block_gas_limit,
            minimum_priority_fee,
            additional_tasks,
            propagate_local_transactions,
        } = self;

        let inner = EthTransactionValidatorInner {
            chain_spec,
            client,
            shanghai,
            eip2718,
            eip1559,
            block_gas_limit,
            minimum_priority_fee,
            propagate_local_transactions,
            _marker: Default::default(),
        };

        let (tx, task) = ValidationTask::new();

        // Spawn validation tasks, they are blocking because they perform db lookups
        for _ in 0..additional_tasks {
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

        EthTransactionValidator { inner: Arc::new(inner), to_validation_task }
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
    /// Toggle to determine if a local transaction should be propagated
    propagate_local_transactions: bool,
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
            // by this point assume all external transactions should be propagated
            propagate: matches!(origin, TransactionOrigin::External) ||
                self.propagate_local_transactions,
        }
    }
}
