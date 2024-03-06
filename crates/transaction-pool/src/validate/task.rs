//! A validation service for transactions.

use crate::{
    blobstore::BlobStore,
    validate::{EthTransactionValidatorBuilder, TransactionValidatorError},
    EthTransactionValidator, PoolTransaction, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator,
};
use futures_util::{lock::Mutex, StreamExt};
use reth_primitives::{ChainSpec, SealedBlock};
use reth_provider::BlockReaderIdExt;
use reth_tasks::TaskSpawner;
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
/// This should be spawned as a task: [ValidationTask::run]
#[derive(Clone)]
pub struct ValidationTask {
    validation_jobs: Arc<Mutex<ValidationStream>>,
}

impl ValidationTask {
    /// Creates a new clonable task pair
    pub fn new() -> (ValidationJobSender, Self) {
        let (tx, rx) = mpsc::channel(1);
        (ValidationJobSender { tx }, Self::with_receiver(rx))
    }

    /// Creates a new task with the given receiver.
    pub fn with_receiver(jobs: mpsc::Receiver<Pin<Box<dyn Future<Output = ()> + Send>>>) -> Self {
        ValidationTask { validation_jobs: Arc::new(Mutex::new(ReceiverStream::new(jobs))) }
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

/// A sender new type for sending validation jobs to [ValidationTask].
#[derive(Debug)]
pub struct ValidationJobSender {
    tx: mpsc::Sender<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl ValidationJobSender {
    /// Sends the given job to the validation task.
    pub async fn send(
        &self,
        job: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Result<(), TransactionValidatorError> {
        self.tx.send(job).await.map_err(|_| TransactionValidatorError::ValidationServiceUnreachable)
    }
}

/// A [TransactionValidator] implementation that validates ethereum transaction.
///
/// This validator is non-blocking, all validation work is done in a separate task.
#[derive(Debug, Clone)]
pub struct TransactionValidationTaskExecutor<V> {
    /// The validator that will validate transactions on a separate task.
    pub validator: V,
    /// The sender half to validation tasks that perform the actual validation.
    pub to_validation_task: Arc<sync::Mutex<ValidationJobSender>>,
}

// === impl TransactionValidationTaskExecutor ===

impl TransactionValidationTaskExecutor<()> {
    /// Convenience method to create a [EthTransactionValidatorBuilder]
    pub fn eth_builder(chain_spec: Arc<ChainSpec>) -> EthTransactionValidatorBuilder {
        EthTransactionValidatorBuilder::new(chain_spec)
    }
}

impl<V> TransactionValidationTaskExecutor<V> {
    /// Maps the given validator to a new type.
    pub fn map<F, T>(self, mut f: F) -> TransactionValidationTaskExecutor<T>
    where
        F: FnMut(V) -> T,
    {
        TransactionValidationTaskExecutor {
            validator: f(self.validator),
            to_validation_task: self.to_validation_task,
        }
    }
}

impl<Client, Tx> TransactionValidationTaskExecutor<EthTransactionValidator<Client, Tx>>
where
    Client: BlockReaderIdExt,
{
    /// Creates a new instance for the given [ChainSpec]
    ///
    /// This will spawn a single validation tasks that performs the actual validation.
    /// See [TransactionValidationTaskExecutor::eth_with_additional_tasks]
    pub fn eth<T, S: BlobStore>(
        client: Client,
        chain_spec: Arc<ChainSpec>,
        blob_store: S,
        tasks: T,
    ) -> Self
    where
        T: TaskSpawner,
    {
        Self::eth_with_additional_tasks(client, chain_spec, blob_store, tasks, 0)
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
    pub fn eth_with_additional_tasks<T, S: BlobStore>(
        client: Client,
        chain_spec: Arc<ChainSpec>,
        blob_store: S,
        tasks: T,
        num_additional_tasks: usize,
    ) -> Self
    where
        T: TaskSpawner,
    {
        EthTransactionValidatorBuilder::new(chain_spec)
            .with_additional_tasks(num_additional_tasks)
            .build_with_tasks::<Client, Tx, T, S>(client, tasks, blob_store)
    }
}

impl<V> TransactionValidationTaskExecutor<V> {
    /// Creates a new executor instance with the given validator for transaction validation.
    ///
    /// Initializes the executor with the provided validator and sets up communication for
    /// validation tasks.
    pub fn new(validator: V) -> Self {
        let (tx, _) = ValidationTask::new();
        Self { validator, to_validation_task: Arc::new(sync::Mutex::new(tx)) }
    }
}

impl<V> TransactionValidator for TransactionValidationTaskExecutor<V>
where
    V: TransactionValidator + Clone + 'static,
{
    type Transaction = <V as TransactionValidator>::Transaction;

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
                let to_validation_task = to_validation_task.lock().await;
                let validator = self.validator.clone();
                to_validation_task
                    .send(Box::pin(async move {
                        let res = validator.validate_transaction(origin, transaction).await;
                        let _ = tx.send(res);
                    }))
                    .await
            };
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

    fn on_new_head_block(&self, new_tip_block: &SealedBlock) {
        self.validator.on_new_head_block(new_tip_block)
    }
}
