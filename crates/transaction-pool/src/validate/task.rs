//! A validation service for transactions.

use crate::{
    blobstore::BlobStore,
    metrics::TxPoolValidatorMetrics,
    validate::{EthTransactionValidatorBuilder, TransactionValidatorError},
    EthTransactionValidator, PoolTransaction, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator,
};
use futures_util::{lock::Mutex, StreamExt};
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::{HeaderTy, SealedBlock};
use reth_storage_api::BlockReaderIdExt;
use reth_tasks::Runtime;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::{
    sync,
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;

/// Represents a future outputting unit type and is sendable.
type ValidationFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Represents a stream of validation futures.
type ValidationStream = ReceiverStream<ValidationFuture>;

/// A service that performs validation jobs.
///
/// This listens for incoming validation jobs and executes them.
///
/// This should be spawned as a task: [`ValidationTask::run`]
#[derive(Clone)]
pub struct ValidationTask {
    validation_jobs: Arc<Mutex<ValidationStream>>,
}

impl ValidationTask {
    /// Creates a new cloneable task pair.
    ///
    /// The sender sends new (transaction) validation tasks to an available validation task.
    pub fn new() -> (ValidationJobSender, Self) {
        Self::with_capacity(1)
    }

    /// Creates a new cloneable task pair with the given channel capacity.
    pub fn with_capacity(capacity: usize) -> (ValidationJobSender, Self) {
        let (tx, rx) = mpsc::channel(capacity);
        let metrics = TxPoolValidatorMetrics::default();
        (ValidationJobSender { tx, metrics }, Self::with_receiver(rx))
    }

    /// Creates a new task with the given receiver.
    pub fn with_receiver(jobs: mpsc::Receiver<Pin<Box<dyn Future<Output = ()> + Send>>>) -> Self {
        Self { validation_jobs: Arc::new(Mutex::new(ReceiverStream::new(jobs))) }
    }

    /// Executes all new validation jobs that come in.
    ///
    /// This will run as long as the channel is alive and is expected to be spawned as a task.
    pub async fn run(self) {
        while let Some(task) = self.validation_jobs.lock().await.next().await {
            task.await;
        }
    }
}

impl std::fmt::Debug for ValidationTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationTask").field("validation_jobs", &"...").finish()
    }
}

/// A sender new type for sending validation jobs to [`ValidationTask`].
#[derive(Debug)]
pub struct ValidationJobSender {
    tx: mpsc::Sender<Pin<Box<dyn Future<Output = ()> + Send>>>,
    metrics: TxPoolValidatorMetrics,
}

impl ValidationJobSender {
    /// Sends the given job to the validation task.
    pub async fn send(
        &self,
        job: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Result<(), TransactionValidatorError> {
        self.metrics.inflight_validation_jobs.increment(1);
        let res = self
            .tx
            .send(job)
            .await
            .map_err(|_| TransactionValidatorError::ValidationServiceUnreachable);
        self.metrics.inflight_validation_jobs.decrement(1);
        res
    }
}

/// A [`TransactionValidator`] implementation that validates ethereum transaction.
/// This validator is non-blocking, all validation work is done in a separate task.
#[derive(Debug)]
pub struct TransactionValidationTaskExecutor<V> {
    /// The validator that will validate transactions on a separate task.
    pub validator: Arc<V>,
    /// The sender half to validation tasks that perform the actual validation.
    pub to_validation_task: Arc<sync::Mutex<ValidationJobSender>>,
}

impl<V> Clone for TransactionValidationTaskExecutor<V> {
    fn clone(&self) -> Self {
        Self {
            validator: self.validator.clone(),
            to_validation_task: self.to_validation_task.clone(),
        }
    }
}

// === impl TransactionValidationTaskExecutor ===

impl TransactionValidationTaskExecutor<()> {
    /// Convenience method to create a [`EthTransactionValidatorBuilder`]
    pub fn eth_builder<Client, Evm>(
        client: Client,
        evm_config: Evm,
    ) -> EthTransactionValidatorBuilder<Client, Evm>
    where
        Client: ChainSpecProvider<ChainSpec: EthereumHardforks>
            + BlockReaderIdExt<Header = HeaderTy<Evm::Primitives>>,
        Evm: ConfigureEvm,
    {
        EthTransactionValidatorBuilder::new(client, evm_config)
    }
}

impl<V> TransactionValidationTaskExecutor<V> {
    /// Maps the given validator to a new type.
    pub fn map<F, T>(self, mut f: F) -> TransactionValidationTaskExecutor<T>
    where
        F: FnMut(V) -> T,
    {
        TransactionValidationTaskExecutor {
            validator: Arc::new(f(Arc::into_inner(self.validator).unwrap())),
            to_validation_task: self.to_validation_task,
        }
    }

    /// Returns the validator.
    pub fn validator(&self) -> &V {
        &self.validator
    }
}

impl<Client, Tx, Evm> TransactionValidationTaskExecutor<EthTransactionValidator<Client, Tx, Evm>> {
    /// Creates a new instance for the given client
    ///
    /// This will spawn a single validation tasks that performs the actual validation.
    /// See [`TransactionValidationTaskExecutor::eth_with_additional_tasks`]
    pub fn eth<S: BlobStore>(client: Client, evm_config: Evm, blob_store: S, tasks: Runtime) -> Self
    where
        Client: ChainSpecProvider<ChainSpec: EthereumHardforks>
            + BlockReaderIdExt<Header = HeaderTy<Evm::Primitives>>,
        Evm: ConfigureEvm,
    {
        Self::eth_with_additional_tasks(client, evm_config, blob_store, tasks, 0)
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
    pub fn eth_with_additional_tasks<S: BlobStore>(
        client: Client,
        evm_config: Evm,
        blob_store: S,
        tasks: Runtime,
        num_additional_tasks: usize,
    ) -> Self
    where
        Client: ChainSpecProvider<ChainSpec: EthereumHardforks>
            + BlockReaderIdExt<Header = HeaderTy<Evm::Primitives>>,
        Evm: ConfigureEvm,
    {
        EthTransactionValidatorBuilder::new(client, evm_config)
            .with_additional_tasks(num_additional_tasks)
            .build_with_tasks(tasks, blob_store)
    }
}

impl<V> TransactionValidationTaskExecutor<V> {
    /// Creates a new executor instance with the given validator for transaction validation.
    ///
    /// Initializes the executor with the provided validator and sets up communication for
    /// validation tasks.
    pub fn new(validator: V) -> (Self, ValidationTask) {
        let (tx, task) = ValidationTask::new();
        (
            Self {
                validator: Arc::new(validator),
                to_validation_task: Arc::new(sync::Mutex::new(tx)),
            },
            task,
        )
    }
}

impl<V> TransactionValidator for TransactionValidationTaskExecutor<V>
where
    V: TransactionValidator + 'static,
{
    type Transaction = <V as TransactionValidator>::Transaction;
    type Block = V::Block;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        let hash = *transaction.hash();
        let (tx, rx) = oneshot::channel();
        {
            let res = {
                let to_validation_task = self.to_validation_task.clone();
                let validator = self.validator.clone();
                let fut = Box::pin(async move {
                    let res = validator.validate_transaction(origin, transaction).await;
                    let _ = tx.send(res);
                });
                let to_validation_task = to_validation_task.lock().await;
                to_validation_task.send(fut).await
            };
            if res.is_err() {
                return TransactionValidationOutcome::Error(
                    hash,
                    Box::new(TransactionValidatorError::ValidationServiceUnreachable),
                );
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

    async fn validate_transactions(
        &self,
        transactions: impl IntoIterator<Item = (TransactionOrigin, Self::Transaction), IntoIter: Send>
            + Send,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        let transactions: Vec<_> = transactions.into_iter().collect();
        let hashes: Vec<_> = transactions.iter().map(|(_, tx)| *tx.hash()).collect();
        let (tx, rx) = oneshot::channel();
        {
            let res = {
                let to_validation_task = self.to_validation_task.clone();
                let validator = self.validator.clone();
                let fut = Box::pin(async move {
                    let res = validator.validate_transactions(transactions).await;
                    let _ = tx.send(res);
                });
                let to_validation_task = to_validation_task.lock().await;
                to_validation_task.send(fut).await
            };
            if res.is_err() {
                return hashes
                    .into_iter()
                    .map(|hash| {
                        TransactionValidationOutcome::Error(
                            hash,
                            Box::new(TransactionValidatorError::ValidationServiceUnreachable),
                        )
                    })
                    .collect();
            }
        }
        match rx.await {
            Ok(res) => res,
            Err(_) => hashes
                .into_iter()
                .map(|hash| {
                    TransactionValidationOutcome::Error(
                        hash,
                        Box::new(TransactionValidatorError::ValidationServiceUnreachable),
                    )
                })
                .collect(),
        }
    }

    fn on_new_head_block(&self, new_tip_block: &SealedBlock<Self::Block>) {
        self.validator.on_new_head_block(new_tip_block)
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
        type Block = reth_ethereum_primitives::Block;

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
