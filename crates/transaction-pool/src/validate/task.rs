//! A validation service for transactions.

use crate::{
    blobstore::BlobStore,
    metrics::TxPoolValidatorMetrics,
    validate::{EthTransactionValidatorBuilder, TransactionValidatorError},
    EthPoolTransaction, EthPooledTransaction, EthTransactionValidator, PoolTransaction,
    TransactionOrigin, TransactionValidationOutcome, TransactionValidator,
};
use futures_util::lock::Mutex;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_primitives_traits::{Block, SealedBlock};
use reth_storage_api::{noop::NoopProvider, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// A validation job for a single transaction.
///
/// Contains the transaction to validate, its origin, and a channel to send the result back.
type ValidationJob<T> = (TransactionOrigin, T, oneshot::Sender<TransactionValidationOutcome<T>>);

/// A service that performs transaction validation jobs.
///
/// This listens for incoming transactions, batches them using `recv_many`,
/// and validates them in batches for improved performance.
///
/// This should be spawned as a task: [`ValidationTask::run`]
#[derive(Debug)]
pub struct ValidationTask<V: TransactionValidator> {
    /// Shared receiver for validation jobs - multiple tasks can compete for work.
    validation_jobs: Arc<Mutex<mpsc::UnboundedReceiver<ValidationJob<V::Transaction>>>>,
    /// The validator used to validate transactions.
    validator: Arc<V>,
    /// Metrics for the validation task.
    metrics: TxPoolValidatorMetrics,
}

impl<V: TransactionValidator> Clone for ValidationTask<V> {
    fn clone(&self) -> Self {
        Self {
            validator: self.validator.clone(),
            validation_jobs: self.validation_jobs.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

impl<V: TransactionValidator> ValidationTask<V> {
    /// Maximum number of transactions to batch in a single validation call.
    const BATCH_SIZE: usize = 64;

    /// Creates a new cloneable task pair.
    ///
    /// The sender sends new (transaction) validation tasks to an available validation task.
    pub fn new(
        validator: Arc<V>,
        metrics: TxPoolValidatorMetrics,
    ) -> (mpsc::UnboundedSender<ValidationJob<V::Transaction>>, Self) {
        let (tx, rx) = mpsc::unbounded_channel();
        (tx, Self { validator, validation_jobs: Arc::new(Mutex::new(rx)), metrics })
    }

    /// Executes validation jobs, batching transactions for efficiency.
    ///
    /// This will run as long as the channel is alive and is expected to be spawned as a task.
    pub async fn run(self) {
        let mut buffer = Vec::with_capacity(Self::BATCH_SIZE);

        loop {
            // Lock the receiver and batch receive transactions
            let count =
                self.validation_jobs.lock().await.recv_many(&mut buffer, Self::BATCH_SIZE).await;

            if count == 0 {
                // Channel closed, exit
                break;
            }

            self.metrics.inflight_validation_jobs.decrement(count as f64);

            // Split into transactions and response senders
            #[expect(clippy::iter_with_drain)]
            let (txs, senders): (Vec<_>, Vec<_>) =
                buffer.drain(..).map(|(origin, tx, sender)| ((origin, tx), sender)).unzip();

            // Batch validate all transactions
            let results = self.validator.validate_transactions(txs).await;

            // Send results back through oneshot channels
            for (result, sender) in results.into_iter().zip(senders) {
                let _ = sender.send(result);
            }
        }
    }
}

/// A [`TransactionValidator`] implementation that validates ethereum transaction.
/// This validator is non-blocking, all validation work is done in a separate task.
#[derive(Debug)]
pub struct TransactionValidationTaskExecutor<V: TransactionValidator> {
    /// The validator that will validate transactions on a separate task.
    pub validator: Arc<V>,
    /// The sender half to validation tasks that perform the actual validation.
    pub to_validation_task: mpsc::UnboundedSender<ValidationJob<V::Transaction>>,
    /// Metrics for the validator task executor.
    pub metrics: TxPoolValidatorMetrics,
}

impl<V: TransactionValidator> Clone for TransactionValidationTaskExecutor<V> {
    fn clone(&self) -> Self {
        Self {
            validator: self.validator.clone(),
            to_validation_task: self.to_validation_task.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

// === impl TransactionValidationTaskExecutor ===

impl
    TransactionValidationTaskExecutor<EthTransactionValidator<NoopProvider, EthPooledTransaction>>
{
    /// Convenience method to create a [`EthTransactionValidatorBuilder`]
    pub fn eth_builder<Client>(client: Client) -> EthTransactionValidatorBuilder<Client> {
        EthTransactionValidatorBuilder::new(client)
    }
}

impl<V: TransactionValidator> TransactionValidationTaskExecutor<V> {
    /// Returns the validator.
    pub fn validator(&self) -> &V {
        &self.validator
    }
}

impl<Client, Tx> TransactionValidationTaskExecutor<EthTransactionValidator<Client, Tx>>
where
    Client: ChainSpecProvider<ChainSpec: EthereumHardforks> + StateProviderFactory + 'static,
    Tx: EthPoolTransaction,
{
    /// Creates a new instance for the given client
    ///
    /// This will spawn a single validation tasks that performs the actual validation.
    /// See [`TransactionValidationTaskExecutor::eth_with_additional_tasks`]
    pub fn eth<T, S: BlobStore>(client: Client, blob_store: S, tasks: T) -> Self
    where
        T: TaskSpawner,
    {
        Self::eth_with_additional_tasks(client, blob_store, tasks, 0)
    }

    /// Creates a new instance for the given client
    ///
    /// By default this will enable support for:
    ///   - shanghai
    ///   - eip1559
    ///   - eip2930
    ///
    /// This will always spawn a validation task that performs the actual validation. It will spawn
    /// `num_additional_tasks` additional tasks.
    pub fn eth_with_additional_tasks<T, S: BlobStore>(
        client: Client,
        blob_store: S,
        tasks: T,
        num_additional_tasks: usize,
    ) -> Self
    where
        T: TaskSpawner,
    {
        EthTransactionValidatorBuilder::new(client)
            .with_additional_tasks(num_additional_tasks)
            .build_with_tasks::<Tx, T, S>(tasks, blob_store)
    }
}

impl<V: TransactionValidator> TransactionValidationTaskExecutor<V> {
    /// Creates a new executor instance with the given validator for transaction validation.
    ///
    /// Initializes the executor with the provided validator and sets up communication for
    /// validation tasks.
    pub fn new(validator: V) -> (Self, ValidationTask<V>) {
        let validator = Arc::new(validator);
        let metrics = TxPoolValidatorMetrics::default();
        let (to_validation_task, task) = ValidationTask::new(validator.clone(), metrics.clone());
        (Self { validator, to_validation_task, metrics }, task)
    }
}

impl<V> TransactionValidator for TransactionValidationTaskExecutor<V>
where
    V: TransactionValidator + 'static,
{
    type Transaction = V::Transaction;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        let hash = *transaction.hash();
        let (tx, rx) = oneshot::channel();

        self.metrics.inflight_validation_jobs.increment(1);
        if self.to_validation_task.send((origin, transaction, tx)).is_err() {
            return TransactionValidationOutcome::Error(
                hash,
                Box::new(TransactionValidatorError::ValidationServiceUnreachable),
            );
        }

        // Wait for the result
        rx.await.unwrap_or_else(|_| {
            TransactionValidationOutcome::Error(
                hash,
                Box::new(TransactionValidatorError::ValidationServiceUnreachable),
            )
        })
    }

    async fn validate_transactions(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        let len = transactions.len();
        if len == 0 {
            return Vec::new();
        }

        // Create oneshot channels for all transactions
        let mut receivers = Vec::with_capacity(len);
        let mut hashes = Vec::with_capacity(len);

        for (origin, transaction) in transactions {
            let hash = *transaction.hash();
            hashes.push(hash);

            let (tx, rx) = oneshot::channel();
            receivers.push((hash, rx));

            self.metrics.inflight_validation_jobs.increment(1);
            if self.to_validation_task.send((origin, transaction, tx)).is_err() {
                return hashes
                    .into_iter()
                    .map(|h| {
                        TransactionValidationOutcome::Error(
                            h,
                            Box::new(TransactionValidatorError::ValidationServiceUnreachable),
                        )
                    })
                    .collect();
            }
        }

        // Collect all results
        let mut results = Vec::with_capacity(len);
        for (hash, rx) in receivers {
            let result = match rx.await {
                Ok(res) => res,
                Err(_) => TransactionValidationOutcome::Error(
                    hash,
                    Box::new(TransactionValidatorError::ValidationServiceUnreachable),
                ),
            };
            results.push(result);
        }

        results
    }

    async fn validate_transactions_with_origin(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = Self::Transaction> + Send,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        self.validate_transactions(transactions.into_iter().map(|tx| (origin, tx)).collect()).await
    }

    fn on_new_head_block<B>(&self, new_tip_block: &SealedBlock<B>)
    where
        B: Block,
    {
        self.validator.on_new_head_block(new_tip_block)
    }
}

/// A builder for [`TransactionValidationTaskExecutor`].
///
/// This builder holds a validator and configuration for spawning validation tasks.
/// Use [`Self::map`] to wrap the validator before spawning tasks.
#[derive(Debug)]
pub struct TransactionValidationTaskExecutorBuilder<V> {
    /// The validator to use for transaction validation.
    validator: V,
    /// Number of additional validation tasks to spawn.
    additional_tasks: usize,
}

impl<V> TransactionValidationTaskExecutorBuilder<V> {
    /// Creates a new builder with the given validator and number of additional tasks.
    pub const fn new(validator: V, additional_tasks: usize) -> Self {
        Self { validator, additional_tasks }
    }

    /// Maps the given validator to a new type.
    pub fn map<F, T>(self, f: F) -> TransactionValidationTaskExecutorBuilder<T>
    where
        F: FnOnce(V) -> T,
    {
        TransactionValidationTaskExecutorBuilder {
            validator: f(self.validator),
            additional_tasks: self.additional_tasks,
        }
    }

    /// Spawns validation tasks and returns the executor.
    pub fn build_and_spawn<S>(self, spawner: S) -> TransactionValidationTaskExecutor<V>
    where
        V: TransactionValidator + 'static,
        S: TaskSpawner,
    {
        let Self { validator, additional_tasks } = self;
        let validator = Arc::new(validator);
        let metrics = TxPoolValidatorMetrics::default();
        let (to_validation_task, task) = ValidationTask::new(validator.clone(), metrics.clone());

        // Spawn additional validation tasks
        for _ in 0..additional_tasks {
            spawner.spawn_blocking(Box::pin(task.clone().run()));
        }

        // Spawn the critical validation task
        spawner.spawn_critical_blocking("transaction-validation-service", Box::pin(task.run()));

        TransactionValidationTaskExecutor { validator, to_validation_task, metrics }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::MockTransaction,
        validate::{TransactionValidationOutcome, ValidTransaction},
        TransactionOrigin,
    };
    use alloy_primitives::{Address, U256};

    #[derive(Debug)]
    struct NoopValidator;

    impl TransactionValidator for NoopValidator {
        type Transaction = MockTransaction;

        async fn validate_transaction(
            &self,
            _origin: TransactionOrigin,
            transaction: Self::Transaction,
        ) -> TransactionValidationOutcome<Self::Transaction> {
            TransactionValidationOutcome::Valid {
                balance: U256::ZERO,
                state_nonce: 0,
                bytecode_hash: None,
                transaction: ValidTransaction::Valid(transaction),
                propagate: false,
                authorities: Some(Vec::<Address>::new()),
            }
        }
    }

    #[tokio::test]
    async fn executor_new_spawns_and_validates_single() {
        let validator = NoopValidator;
        let (executor, task) = TransactionValidationTaskExecutor::new(validator);
        tokio::spawn(task.run());
        let tx = MockTransaction::legacy();
        let out = executor.validate_transaction(TransactionOrigin::External, tx).await;
        assert!(matches!(out, TransactionValidationOutcome::Valid { .. }));
    }

    #[tokio::test]
    async fn executor_new_spawns_and_validates_batch() {
        let validator = NoopValidator;
        let (executor, task) = TransactionValidationTaskExecutor::new(validator);
        tokio::spawn(task.run());
        let txs = vec![
            (TransactionOrigin::External, MockTransaction::legacy()),
            (TransactionOrigin::Local, MockTransaction::legacy()),
        ];
        let out = executor.validate_transactions(txs).await;
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|o| matches!(o, TransactionValidationOutcome::Valid { .. })));
    }
}
