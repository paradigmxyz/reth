//! Bounded transaction admission shared by RPC and gossip.

use crate::{
    error::PoolError, AddedTransactionOutcome, PoolTransaction, RawPoolTransactionError,
    TransactionOrigin, TransactionValidationOutcome, TransactionValidator,
};
use alloy_primitives::Bytes;
use futures_util::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use reth_evm::SenderRecoveryCache;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_primitives_traits::InMemorySize;
use reth_tasks::pause::TaskPause;
use std::{fmt, sync::Arc, time::Instant};
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};

/// A cloneable admission handle. RPC and gossip have separate bounded queues and share workers.
#[derive(Debug)]
pub struct TransactionIngress<T: PoolTransaction> {
    lanes: [IngressLane<T>; 2],
    context: Arc<IngressContext>,
    // Only public handles keep the service alive; paused jobs must not retain this sender.
    lifetime: watch::Sender<()>,
}

impl<T: PoolTransaction> Clone for TransactionIngress<T> {
    fn clone(&self) -> Self {
        Self {
            lanes: self.lanes.clone(),
            context: self.context.clone(),
            lifetime: self.lifetime.clone(),
        }
    }
}

impl<T: PoolTransaction + 'static> TransactionIngress<T> {
    /// Starts a shared scheduler on the current Tokio runtime.
    ///
    /// `process` receives bounded batches and must complete or drop every request. It must
    /// offload blocking work, for example through [`TransactionValidator::try_dispatch_ingress`].
    /// Avoid capturing a strong reference to an owner that also stores this handle.
    pub fn new(
        config: TransactionIngressConfig,
        concurrency: usize,
        cache: Option<SenderRecoveryCache>,
        process: impl Fn(Vec<IngressRequest<T>>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Self {
        let (rpc_tx, rpc_rx) = mpsc::unbounded_channel();
        let (p2p_tx, p2p_rx) = mpsc::unbounded_channel();
        let lane = |sender, source| IngressLane {
            sender,
            metrics: IngressMetrics::new_with_labels(&[("source", source)]),
            slots: Arc::new(Semaphore::new(config.max_transactions.min(Semaphore::MAX_PERMITS))),
            bytes: Arc::new(Semaphore::new(config.max_bytes.min(u32::MAX as usize))),
        };
        let (lifetime, shutdown) = watch::channel(());
        let context = Arc::new(IngressContext { cache, pause: TaskPause::default(), shutdown });
        let ingress = Self {
            lanes: [lane(rpc_tx, "rpc"), lane(p2p_tx, "p2p")],
            context: context.clone(),
            lifetime,
        };
        tokio::spawn(run(
            [rpc_rx, p2p_rx],
            config.max_batch_size.max(1).min(config.max_transactions.max(1)),
            config.max_batch_bytes.max(1),
            concurrency.max(1),
            context,
            process,
        ));
        ingress
    }

    /// Returns the shared trigger used to pause ingress during foreground CPU work.
    pub fn pause_handle(&self) -> TaskPause {
        self.context.pause.clone()
    }

    /// Reserves RPC capacity before asynchronous preparation, without waiting for space.
    /// The caller must include all retained input and keep the permit until work ends.
    pub fn reserve_rpc(&self, bytes: usize) -> Result<IngressPermit, IngressError> {
        self.reserve(0, bytes)
    }

    /// Tries to admit gossip without waiting or retaining additional work outside the budget.
    pub fn try_submit_pooled(
        &self,
        transaction: T::Pooled,
    ) -> Result<IngressResultReceiver<T>, IngressError> {
        let permit = self.reserve(1, transaction.size())?;
        self.send(
            1,
            TransactionOrigin::External,
            IngressInput::Pooled(transaction),
            permit,
            false,
            None,
        )
    }

    /// Admits an already recovered transaction from RPC or an internal caller.
    pub fn submit_recovered(
        &self,
        origin: TransactionOrigin,
        transaction: T,
    ) -> Result<IngressResultReceiver<T>, IngressError> {
        let permit = self.reserve_rpc(transaction.ingress_size())?;
        self.send(0, origin, IngressInput::Recovered(transaction), permit, false, None)
    }

    /// Decodes, recovers, validates and inserts raw RPC input under one admission permit.
    /// The callback runs after successful recovery, before validation, for RPC subscriptions.
    pub fn submit_raw(
        &self,
        origin: TransactionOrigin,
        transaction: Bytes,
        on_recovered: impl FnOnce() + Send + 'static,
    ) -> Result<IngressResultReceiver<T>, IngressError> {
        let permit = self.reserve(0, transaction.len())?;
        self.send(
            0,
            origin,
            IngressInput::Raw(transaction),
            permit,
            false,
            Some(Box::new(on_recovered)),
        )
    }

    /// Recovers input while keeping its permit for adapter-specific asynchronous preparation.
    pub fn recover_raw(
        &self,
        transaction: Bytes,
    ) -> Result<IngressResultReceiver<T>, IngressError> {
        let permit = self.reserve(0, transaction.len())?;
        self.send(0, TransactionOrigin::Local, IngressInput::Raw(transaction), permit, true, None)
    }

    /// Continues an admitted transaction after adapter-specific preparation, without reacquiring
    /// capacity. The same permit covers both phases, including cancellation of the RPC caller.
    pub fn submit_admitted(
        &self,
        origin: TransactionOrigin,
        transaction: T,
        permit: IngressPermit,
    ) -> Result<IngressResultReceiver<T>, IngressError> {
        let mut permit = permit;
        if !Arc::ptr_eq(permit.0._slots.semaphore(), &self.lanes[0].slots) {
            return Err(IngressError::Closed)
        }
        // Sidecar conversion can expand an admitted RPC input. Charge the extra memory before
        // it re-enters the queue, without releasing its original count reservation.
        permit.grow(transaction.ingress_size())?;
        self.send(0, origin, IngressInput::Recovered(transaction), permit, false, None)
    }

    fn reserve(&self, lane: usize, bytes: usize) -> Result<IngressPermit, IngressError> {
        let result = (|| {
            let slots = self.lanes[lane]
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| IngressError::Full)?;
            let bytes = u32::try_from(bytes.max(1)).map_err(|_| IngressError::Full)?;
            let bytes = self.lanes[lane]
                .bytes
                .clone()
                .try_acquire_many_owned(bytes)
                .map_err(|_| IngressError::Full)?;
            let metrics = self.lanes[lane].metrics.clone();
            metrics.admitted.increment(1);
            metrics.outstanding.increment(1.0);
            metrics.outstanding_bytes.increment(bytes.num_permits() as f64);
            Ok(IngressPermit(PermitInner { _slots: slots, _bytes: bytes, metrics }))
        })();
        if result.is_err() {
            self.lanes[lane].metrics.rejected.increment(1);
        }
        result
    }

    fn send(
        &self,
        lane: usize,
        origin: TransactionOrigin,
        input: IngressInput<T>,
        permit: IngressPermit,
        recover_only: bool,
        on_recovered: Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<IngressResultReceiver<T>, IngressError> {
        let (response, rx) = oneshot::channel();
        self.lanes[lane]
            .sender
            .send(IngressRequest {
                origin,
                input,
                permit,
                recover_only,
                on_recovered,
                response,
                queued_at: Instant::now(),
                context: self.context.clone(),
            })
            .map_err(|_| IngressError::Closed)?;
        Ok(rx)
    }
}

/// Limits apply separately to RPC and gossip, including queued and executing transactions.
#[derive(Debug, Clone, Copy)]
pub struct TransactionIngressConfig {
    /// Maximum admitted transactions per lane.
    pub max_transactions: usize,
    /// Maximum estimated input bytes per lane.
    pub max_bytes: usize,
    /// Maximum transactions per recovery/validation/insertion job.
    pub max_batch_size: usize,
    /// Maximum estimated input bytes in a batch; one larger transaction runs alone.
    pub max_batch_bytes: usize,
}

impl Default for TransactionIngressConfig {
    fn default() -> Self {
        Self {
            max_transactions: 4096,
            max_bytes: 32 * 1024 * 1024,
            max_batch_size: 32,
            max_batch_bytes: 256 * 1024,
        }
    }
}

/// Admission, recovery, or pool insertion failure.
#[derive(Debug, thiserror::Error)]
pub enum IngressError {
    /// Local overload, never a peer fault or a permanently invalid transaction.
    #[error("transaction ingress capacity exhausted")]
    Full,
    /// The service or validation worker shut down.
    #[error("transaction ingress service unavailable")]
    Closed,
    /// Raw decoding or sender recovery failure.
    #[error(transparent)]
    Recovery(#[from] RawPoolTransactionError),
    /// Validation or insertion failure.
    #[error(transparent)]
    Pool(#[from] PoolError),
}

/// A terminal insertion result, or a recovered transaction awaiting adapter preparation.
#[derive(Debug)]
pub enum IngressOutcome<T: PoolTransaction> {
    /// Transaction insertion completed.
    Inserted(AddedTransactionOutcome),
    /// The adapter must keep the permit until submission or abandonment.
    Recovered(T, IngressPermit),
}

/// Completion for one admitted input, independent of its original transport batch.
pub type IngressResultReceiver<T> = oneshot::Receiver<Result<IngressOutcome<T>, IngressError>>;

/// Owns admission capacity until all work using it has ended.
#[derive(Debug)]
pub struct IngressPermit(PermitInner);

impl IngressPermit {
    /// Raises the retained-byte reservation before input expands, without waiting for space.
    /// Failure leaves the existing reservation intact until the permit is dropped.
    pub fn grow(&mut self, bytes: usize) -> Result<(), IngressError> {
        let extra = bytes.saturating_sub(self.0._bytes.num_permits());
        if extra > 0 {
            let extra = u32::try_from(extra).map_err(|_| IngressError::Full)?;
            let more = self
                .0
                ._bytes
                .semaphore()
                .clone()
                .try_acquire_many_owned(extra)
                .map_err(|_| IngressError::Full)?;
            self.0.metrics.outstanding_bytes.increment(extra as f64);
            self.0._bytes.merge(more);
        }
        Ok(())
    }
}

/// Input owned by the validation job, including admission and response ownership.
///
/// Validators overriding `try_dispatch_ingress` must preserve one result per request. Moving
/// requests into the worker keeps permits alive even if the submitting task is canceled.
pub struct IngressRequest<T: PoolTransaction> {
    pub(crate) origin: TransactionOrigin,
    context: Arc<IngressContext>,
    queued_at: Instant,
    input: IngressInput<T>,
    pub(crate) permit: IngressPermit,
    recover_only: bool,
    on_recovered: Option<Box<dyn FnOnce() + Send>>,
    response: oneshot::Sender<Result<IngressOutcome<T>, IngressError>>,
}

impl<T: PoolTransaction> fmt::Debug for IngressRequest<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IngressRequest")
            .field("origin", &self.origin)
            .field("recover_only", &self.recover_only)
            .finish_non_exhaustive()
    }
}

/// A validated input with its admission and response ownership retained through insertion.
#[derive(Debug)]
pub struct ValidatedIngress<T: PoolTransaction> {
    pub(crate) origin: TransactionOrigin,
    pub(crate) outcome: TransactionValidationOutcome<T>,
    pub(crate) permit: IngressPermit,
    pub(crate) response: oneshot::Sender<Result<IngressOutcome<T>, IngressError>>,
}

impl<T: PoolTransaction> ValidatedIngress<T> {
    /// Separates the validated transaction from its response and admission ownership.
    pub fn into_parts(
        self,
    ) -> (TransactionOrigin, TransactionValidationOutcome<T>, IngressCompletion<T>) {
        (
            self.origin,
            self.outcome,
            IngressCompletion { _permit: self.permit, response: self.response },
        )
    }
}

/// Retains admission until the pool has finished inserting a validated input.
#[derive(Debug)]
pub struct IngressCompletion<T: PoolTransaction> {
    _permit: IngressPermit,
    response: oneshot::Sender<Result<IngressOutcome<T>, IngressError>>,
}

impl<T: PoolTransaction> IngressCompletion<T> {
    /// Returns the insertion outcome to the adapter and releases the admission permit.
    pub fn complete(self, result: Result<AddedTransactionOutcome, PoolError>) {
        let _ =
            self.response.send(result.map(IngressOutcome::Inserted).map_err(IngressError::Pool));
    }
}

/// Callback run on the validation worker after recovery and state validation.
pub type IngressBatchCompletion<T> = Box<dyn FnOnce(Vec<ValidatedIngress<T>>) + Send>;

/// An owned recovery/validation/insertion job for a validation worker.
pub struct IngressBatch<T: PoolTransaction> {
    requests: Vec<IngressRequest<T>>,
    complete: IngressBatchCompletion<T>,
}

impl<T: PoolTransaction> IngressBatch<T> {
    /// Combines admitted inputs and their insertion callback into one worker job.
    pub fn new(requests: Vec<IngressRequest<T>>, complete: IngressBatchCompletion<T>) -> Self {
        Self { requests, complete }
    }

    /// Runs all CPU work, including insertion. Call this on a blocking or validation worker.
    pub async fn run<V: TransactionValidator<Transaction = T> + ?Sized>(self, validator: &V) {
        let Some(context) = self.requests.first().map(|request| request.context.clone()) else {
            return
        };
        let validated = validate_ingress(validator, self.requests).await;
        if !validated.is_empty() && context.ready().await {
            (self.complete)(validated);
        }
    }
}

impl<T: PoolTransaction> fmt::Debug for IngressBatch<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IngressBatch")
            .field("requests", &self.requests.len())
            .finish_non_exhaustive()
    }
}

/// Recovers a bounded batch before opening the validator's state provider.
pub async fn validate_ingress<V: TransactionValidator + ?Sized>(
    validator: &V,
    requests: Vec<IngressRequest<V::Transaction>>,
) -> Vec<ValidatedIngress<V::Transaction>> {
    let Some(context) = requests.first().map(|request| request.context.clone()) else {
        return Vec::new()
    };
    let mut transactions = Vec::with_capacity(requests.len());
    let mut completions = Vec::with_capacity(requests.len());
    for request in requests {
        let IngressRequest {
            origin,
            input,
            mut permit,
            recover_only,
            on_recovered,
            response,
            queued_at,
            context: _,
        } = request;
        if !context.ready().await {
            return Vec::new()
        }
        if response.is_closed() {
            continue
        }
        permit.0.metrics.queue_duration.record(queued_at.elapsed());
        let recovery_start = Instant::now();
        // RPC subscriptions and asynchronous preparation can retain the original encoding
        // alongside the decoded transaction and its sidecar.
        let raw_bytes = match &input {
            IngressInput::Raw(bytes) => bytes.len(),
            _ => 0,
        };
        let recovered = match input {
            IngressInput::Raw(bytes) => match &context.cache {
                Some(cache) => {
                    V::Transaction::recover_raw_transaction_with_cache(&bytes, Some(cache))
                }
                None => V::Transaction::recover_raw_transaction(&bytes),
            },
            IngressInput::Pooled(tx) => match &context.cache {
                Some(cache) => V::Transaction::try_recover_with_cache(tx, cache),
                None => V::Transaction::try_recover(tx),
            }
            .map_err(|_| RawPoolTransactionError::InvalidTransactionSignature),
            IngressInput::Recovered(tx) => Ok(tx),
        };
        permit.0.metrics.recovery_duration.record(recovery_start.elapsed());
        match recovered {
            Err(error) => {
                let _ = response.send(Err(IngressError::Recovery(error)));
            }
            Ok(transaction) => {
                if let Err(error) =
                    permit.grow(transaction.ingress_size().saturating_add(raw_bytes))
                {
                    let _ = response.send(Err(error));
                    continue
                }
                if let Some(callback) = on_recovered {
                    callback();
                }
                if recover_only {
                    let _ = response.send(Ok(IngressOutcome::Recovered(transaction, permit)));
                } else {
                    transactions.push((origin, transaction));
                    completions.push((origin, permit, response));
                }
            }
        }
    }
    if transactions.is_empty() || !context.ready().await {
        return Vec::new()
    }
    let origin = transactions[0].0;
    let outcomes = if transactions.iter().all(|(other, _)| *other == origin) {
        validator
            .validate_transactions_with_origin(origin, transactions.into_iter().map(|(_, tx)| tx))
            .await
    } else {
        validator.validate_transactions(transactions).await
    };
    assert_eq!(outcomes.len(), completions.len(), "validator must return one outcome per input");
    completions
        .into_iter()
        .zip(outcomes)
        .map(|((origin, permit, response), outcome)| ValidatedIngress {
            origin,
            outcome,
            permit,
            response,
        })
        .collect()
}

#[derive(Debug)]
struct IngressLane<T: PoolTransaction> {
    sender: mpsc::UnboundedSender<IngressRequest<T>>,
    metrics: IngressMetrics,
    slots: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
}

impl<T: PoolTransaction> Clone for IngressLane<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            slots: self.slots.clone(),
            bytes: self.bytes.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

#[derive(Debug)]
struct PermitInner {
    _slots: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
    metrics: IngressMetrics,
}

impl Drop for PermitInner {
    fn drop(&mut self) {
        self.metrics.outstanding.decrement(1.0);
        self.metrics.outstanding_bytes.decrement(self._bytes.num_permits() as f64);
    }
}

#[derive(Clone, Metrics)]
#[metrics(scope = "transaction_pool.ingress")]
struct IngressMetrics {
    /// Successfully admitted transactions.
    admitted: Counter,
    /// Requests rejected before queueing due to local capacity.
    rejected: Counter,
    /// Queued, executing or adapter-prepared transactions retaining admission.
    outstanding: Gauge,
    /// Estimated bytes of admitted input.
    outstanding_bytes: Gauge,
    /// Time from enqueue to recovery or validation start.
    queue_duration: Histogram,
    /// Time spent decoding and recovering an input.
    recovery_duration: Histogram,
    /// Number of transactions dispatched together.
    batch_size: Histogram,
}

#[derive(Debug)]
enum IngressInput<T: PoolTransaction> {
    Raw(Bytes),
    Pooled(T::Pooled),
    Recovered(T),
}

#[derive(Debug)]
struct IngressContext {
    cache: Option<SenderRecoveryCache>,
    pause: TaskPause,
    shutdown: watch::Receiver<()>,
}

impl IngressContext {
    /// Closing every admission handle cancels paused work as well as the scheduler.
    async fn ready(&self) -> bool {
        let mut shutdown = self.shutdown.clone();
        tokio::select! {
            biased;
            _ = shutdown.changed() => false,
            _ = self.pause.resumed() => true,
        }
    }
}

async fn run<T: PoolTransaction>(
    mut lanes: [mpsc::UnboundedReceiver<IngressRequest<T>>; 2],
    max_batch: usize,
    max_batch_bytes: usize,
    concurrency: usize,
    context: Arc<IngressContext>,
    process: impl Fn(Vec<IngressRequest<T>>) -> BoxFuture<'static, ()>,
) {
    let mut jobs = FuturesUnordered::new();
    let mut next_lane = 0;
    let mut closed = [false; 2];
    let mut held = [None, None];
    loop {
        // Completed jobs release admission before another input batch is selected.
        while jobs.len() >= concurrency {
            jobs.next().await;
        }
        if !context.ready().await {
            break
        }
        let first = futures_util::future::poll_fn(|cx| {
            while jobs.poll_next_unpin(cx) == std::task::Poll::Ready(Some(())) {}
            for offset in 0..2 {
                let lane = (next_lane + offset) % 2;
                if let Some(request) = held[lane].take() {
                    return std::task::Poll::Ready(Some((lane, request)))
                }
                match lanes[lane].poll_recv(cx) {
                    std::task::Poll::Ready(Some(request)) => {
                        return std::task::Poll::Ready(Some((lane, request)))
                    }
                    std::task::Poll::Ready(None) => closed[lane] = true,
                    std::task::Poll::Pending => {}
                }
            }
            if closed.iter().all(|closed| *closed) && jobs.is_empty() {
                std::task::Poll::Ready(None)
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
        let Some((lane, first)) = first else { break };
        next_lane = 1 - lane;
        let mut batch = Vec::with_capacity(max_batch);
        let mut bytes = first.permit.0._bytes.num_permits();
        let metrics = first.permit.0.metrics.clone();
        batch.push(first);
        while batch.len() < max_batch {
            match lanes[lane].try_recv() {
                Ok(request) => {
                    let size = request.permit.0._bytes.num_permits();
                    if bytes.saturating_add(size) > max_batch_bytes {
                        held[lane] = Some(request);
                        break
                    }
                    bytes += size;
                    batch.push(request);
                }
                Err(_) => break,
            }
        }
        metrics.batch_size.record(batch.len() as f64);
        jobs.push(process(batch));
        // Start each selected batch promptly, without intentionally delaying low-load traffic.
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blobstore::InMemoryBlobStore,
        test_utils::{MockTransaction, OkValidator, TransactionGenerator},
        CoinbaseTipOrdering, EthPooledTransaction, Pool, PoolConfig, TransactionPool,
        TransactionValidationTaskExecutor,
    };
    use alloy_consensus::TxEip1559;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Signature, U256};
    use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
    use std::time::Duration;

    fn enqueue(
        ingress: &TransactionIngress<MockTransaction>,
        lane: usize,
        nonce: u64,
    ) -> IngressResultReceiver<MockTransaction> {
        let tx = MockTransaction::eip1559().with_nonce(nonce);
        let permit = ingress.reserve(lane, tx.size()).unwrap();
        ingress
            .send(
                lane,
                if lane == 0 { TransactionOrigin::Local } else { TransactionOrigin::External },
                IngressInput::Recovered(tx),
                permit,
                false,
                None,
            )
            .unwrap()
    }

    #[tokio::test]
    async fn canceled_call_retains_capacity_until_running_work_ends() {
        let (started, mut batches) = mpsc::unbounded_channel();
        let ingress = TransactionIngress::new(
            TransactionIngressConfig { max_transactions: 1, ..Default::default() },
            1,
            None,
            move |requests: Vec<IngressRequest<MockTransaction>>| {
                let (release, wait) = oneshot::channel();
                started.send(release).unwrap();
                Box::pin(async move {
                    let _ = wait.await;
                    drop(requests);
                })
            },
        );
        let response = enqueue(&ingress, 0, 0);
        let release = batches.recv().await.unwrap();
        drop(response);
        assert!(matches!(ingress.reserve(0, 1), Err(IngressError::Full)));
        // Gossip retains independent admission while RPC is saturated.
        let gossip = ingress.reserve(1, 1).unwrap();
        drop(gossip);
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if ingress.reserve(0, 1).is_ok() {
                    break
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn byte_budget_is_released_on_failed_count_or_byte_admission() {
        let ingress = TransactionIngress::<MockTransaction>::new(
            TransactionIngressConfig {
                max_transactions: 2,
                max_bytes: 10,
                max_batch_size: 2,
                ..Default::default()
            },
            1,
            None,
            |_| Box::pin(async {}),
        );
        let first = ingress.reserve(0, 8).unwrap();
        assert!(matches!(ingress.reserve(0, 3), Err(IngressError::Full)));
        let second = ingress.reserve(0, 2).unwrap();
        assert!(matches!(ingress.reserve(0, 1), Err(IngressError::Full)));
        drop((first, second));
        assert!(ingress.reserve(0, 10).is_ok());
    }

    #[tokio::test]
    async fn recovered_sidecars_and_preparation_growth_respect_byte_budget() {
        use crate::EthPoolTransaction;
        use alloy_eips::{
            eip4844::{Blob, BlobTransactionSidecar},
            eip7594::BlobTransactionSidecarEip7594,
        };

        let base = TransactionGenerator::new(rand::rng()).gen_eip4844_pooled();
        for sidecar in [
            BlobTransactionSidecar { blobs: vec![Blob::ZERO], ..Default::default() }.into(),
            BlobTransactionSidecarEip7594 { blobs: vec![Blob::ZERO], ..Default::default() }.into(),
        ] {
            let transaction =
                EthPooledTransaction::try_from_eip4844(base.clone().into_consensus(), sidecar)
                    .unwrap();
            let bytes = transaction.ingress_size() - 1;
            assert!(transaction.size() < bytes);
            let ingress = TransactionIngress::new(
                TransactionIngressConfig { max_bytes: bytes, ..Default::default() },
                1,
                None,
                |_| panic!("over-budget input must never reach a worker"),
            );
            assert!(matches!(
                ingress.submit_recovered(TransactionOrigin::Local, transaction.clone()),
                Err(IngressError::Full)
            ));
            let mut permit = ingress.reserve_rpc(transaction.size()).unwrap();
            assert!(matches!(permit.grow(transaction.ingress_size()), Err(IngressError::Full)));
            assert_eq!(ingress.lanes[0].bytes.available_permits(), bytes - transaction.size());
            assert!(matches!(
                ingress.submit_admitted(TransactionOrigin::Local, transaction, permit),
                Err(IngressError::Full)
            ));
            assert_eq!(ingress.lanes[0].bytes.available_permits(), bytes);
        }
    }

    #[tokio::test]
    async fn fair_batches_use_all_workers_and_do_not_wait_for_a_full_batch() {
        let (started, mut batches) = mpsc::unbounded_channel();
        let ingress = TransactionIngress::new(
            TransactionIngressConfig { max_batch_size: 2, ..Default::default() },
            2,
            None,
            move |requests: Vec<IngressRequest<MockTransaction>>| {
                let origins = requests.iter().map(|request| request.origin).collect::<Vec<_>>();
                let (release, wait) = oneshot::channel();
                started.send((origins, release)).unwrap();
                Box::pin(async move {
                    let _ = wait.await;
                    drop(requests);
                })
            },
        );
        let mut responses = Vec::new();
        for nonce in 0..8 {
            responses.push(enqueue(&ingress, 0, nonce));
        }
        responses.push(enqueue(&ingress, 1, 0));
        let (first, release_first) = batches.recv().await.unwrap();
        let (second, release_second) =
            tokio::time::timeout(Duration::from_secs(2), batches.recv()).await.unwrap().unwrap();
        assert_eq!(first, vec![TransactionOrigin::Local; 2]);
        assert_eq!(second, vec![TransactionOrigin::External]);
        // Both slots are occupied; the RPC backlog must not spawn more jobs.
        assert!(batches.try_recv().is_err());
        release_second.send(()).unwrap();
        let (third, release_third) = batches.recv().await.unwrap();
        assert_eq!(third, vec![TransactionOrigin::Local; 2]);
        drop((release_first, release_third, responses));
    }

    #[tokio::test]
    async fn batches_respect_byte_limit_and_run_oversized_inputs_alone() {
        let (started, mut batches) = mpsc::unbounded_channel();
        let ingress = TransactionIngress::new(
            TransactionIngressConfig {
                max_batch_size: 8,
                max_batch_bytes: 10,
                ..Default::default()
            },
            1,
            None,
            move |requests: Vec<IngressRequest<MockTransaction>>| {
                let sizes =
                    requests.iter().map(|r| r.permit.0._bytes.num_permits()).collect::<Vec<_>>();
                let started = started.clone();
                Box::pin(async move {
                    started.send(sizes).unwrap();
                    drop(requests);
                })
            },
        );
        let mut responses = Vec::new();
        for size in [6, 6, 20, 4, 4] {
            let permit = ingress.reserve(0, size).unwrap();
            responses.push(
                ingress
                    .send(
                        0,
                        TransactionOrigin::Local,
                        IngressInput::Recovered(MockTransaction::eip1559()),
                        permit,
                        false,
                        None,
                    )
                    .unwrap(),
            );
        }
        for expected in [vec![6], vec![6], vec![20], vec![4, 4]] {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), batches.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                expected
            );
        }
        drop(responses);
    }

    #[tokio::test]
    async fn prepared_permits_cannot_be_transferred_between_pools() {
        let config = TransactionIngressConfig { max_transactions: 1, ..Default::default() };
        let first =
            TransactionIngress::<MockTransaction>::new(config, 1, None, |_| Box::pin(async {}));
        let second =
            TransactionIngress::<MockTransaction>::new(config, 1, None, |_| Box::pin(async {}));
        let permit = first.reserve(0, 1).unwrap();
        assert!(matches!(
            second.submit_admitted(TransactionOrigin::Local, MockTransaction::eip1559(), permit),
            Err(IngressError::Closed)
        ));
        assert!(first.reserve(0, 1).is_ok());
        assert!(second.reserve(0, 1).is_ok());
    }

    #[tokio::test]
    async fn raw_and_pooled_recovery_preserve_results_and_origins() {
        let (executor, worker) =
            TransactionValidationTaskExecutor::new(OkValidator::<EthPooledTransaction>::default());
        let worker = tokio::spawn(worker.run());
        let pool = Pool::new(
            executor,
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            PoolConfig::default(),
        );
        let ingress = pool.transaction_ingress().unwrap();
        let mut generator = TransactionGenerator::new(rand::rng());
        let raw = generator.gen_eip1559();
        let raw_hash = *raw.tx_hash();
        let pooled =
            PooledTransactionVariant::try_from(generator.transaction().nonce(1).into_eip1559())
                .unwrap();
        let pooled_hash = *pooled.tx_hash();
        let invalid = PooledTransactionVariant::try_from(TransactionSigned::new_unhashed(
            TxEip1559::default().into(),
            Signature::new(U256::ZERO, U256::ZERO, false),
        ))
        .unwrap();
        let rpc = ingress
            .submit_raw(TransactionOrigin::Private, raw.encoded_2718().into(), || {})
            .unwrap();
        let p2p = ingress.try_submit_pooled(pooled).unwrap();
        let bad = ingress.try_submit_pooled(invalid).unwrap();
        assert!(
            matches!(rpc.await.unwrap().unwrap(), IngressOutcome::Inserted(outcome) if outcome.hash == raw_hash)
        );
        assert!(
            matches!(p2p.await.unwrap().unwrap(), IngressOutcome::Inserted(outcome) if outcome.hash == pooled_hash)
        );
        assert!(matches!(
            bad.await.unwrap(),
            Err(IngressError::Recovery(RawPoolTransactionError::InvalidTransactionSignature))
        ));
        assert_eq!(pool.get(&raw_hash).unwrap().origin, TransactionOrigin::Private);
        assert_eq!(pool.get(&pooled_hash).unwrap().origin, TransactionOrigin::External);
        let weak_pool = Arc::downgrade(&pool.pool);
        drop((ingress, pool));
        tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap();
        assert!(weak_pool.upgrade().is_none(), "idle ingress must not keep the pool alive");
    }

    #[tokio::test]
    async fn pause_between_recoveries_retains_admission_and_allows_shutdown() {
        for shutdown in [false, true] {
            let cache = SenderRecoveryCache::new(16);
            let (executor, worker) = TransactionValidationTaskExecutor::new(OkValidator::<
                EthPooledTransaction,
            >::default(
            ));
            let worker = tokio::spawn(worker.run());
            let pool = Pool::new(
                executor,
                CoinbaseTipOrdering::default(),
                InMemoryBlobStore::default(),
                PoolConfig {
                    ingress: TransactionIngressConfig { max_transactions: 2, ..Default::default() },
                    ..Default::default()
                },
            )
            .with_sender_recovery_cache(Some(cache.clone()));
            let ingress = pool.transaction_ingress().unwrap();
            let slots = ingress.lanes[0].slots.clone();
            let pause = ingress.pause_handle();
            let initial = pause.pause();
            let overlap = pause.pause();
            let mut generator = TransactionGenerator::new(rand::rng());
            let first = generator.transaction().into_legacy();
            let second = generator.transaction().nonce(1).into_eip1559();
            let (paused_tx, paused_rx) = oneshot::channel();
            let first_result = ingress
                .submit_raw(TransactionOrigin::Local, first.encoded_2718().into(), move || {
                    let _ = paused_tx.send(pause.pause());
                })
                .unwrap();
            let second_result = ingress
                .submit_raw(TransactionOrigin::Local, second.encoded_2718().into(), || {})
                .unwrap();
            drop(initial);
            tokio::task::yield_now().await;
            assert_eq!(cache.get(first.tx_hash()), None);
            assert!(matches!(ingress.reserve(0, 1), Err(IngressError::Full)));
            drop(overlap);
            let between =
                tokio::time::timeout(Duration::from_secs(2), paused_rx).await.unwrap().unwrap();
            assert!(cache.get(first.tx_hash()).is_some());
            assert_eq!(cache.get(second.tx_hash()), None);
            assert!(pool.is_empty());
            assert_eq!(slots.available_permits(), 0);
            if shutdown {
                drop((ingress, pool));
                // Even a foreground producer that outlives the pool cannot strand paused jobs.
                tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap();
                assert!(first_result.await.is_err());
                assert!(second_result.await.is_err());
                drop(between);
            } else {
                drop(between);
                first_result.await.unwrap().unwrap();
                second_result.await.unwrap().unwrap();
                assert!(cache.get(second.tx_hash()).is_some());
                drop((ingress, pool));
                tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap();
            }
            assert_eq!(slots.available_permits(), 2);
        }
    }

    #[tokio::test]
    async fn raw_and_pooled_cache_publication_precedes_rejected_validation() {
        use reth_primitives_traits::{
            transaction::error::InvalidTransactionError, SignedTransaction,
        };

        #[derive(Debug)]
        struct RejectValidator(SenderRecoveryCache);
        impl TransactionValidator for RejectValidator {
            type Transaction = EthPooledTransaction;
            type Block = reth_ethereum_primitives::Block;

            async fn validate_transaction(
                &self,
                _origin: TransactionOrigin,
                transaction: Self::Transaction,
            ) -> TransactionValidationOutcome<Self::Transaction> {
                assert_eq!(self.0.get(transaction.hash()), Some(transaction.sender()));
                TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidTransactionError::ChainIdMismatch.into(),
                )
            }
        }

        let cache = SenderRecoveryCache::new(16);
        let pool = Pool::new(
            RejectValidator(cache.clone()),
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            PoolConfig::default(),
        )
        .with_sender_recovery_cache(Some(cache.clone()));
        let ingress = pool.transaction_ingress().unwrap();
        let mut generator = TransactionGenerator::new(rand::rng());
        for transaction in
            [generator.transaction().chain_id(1).into_legacy(), generator.gen_eip1559()]
        {
            let expected = transaction.try_recover().unwrap();
            let raw = ingress
                .submit_raw(TransactionOrigin::Local, transaction.encoded_2718().into(), || {})
                .unwrap();
            assert!(matches!(raw.await.unwrap(), Err(IngressError::Pool(_))));
            assert_eq!(cache.get(transaction.tx_hash()), Some(expected));
            let pooled = ingress.try_submit_pooled(transaction.try_into().unwrap()).unwrap();
            assert!(matches!(pooled.await.unwrap(), Err(IngressError::Pool(_))));
        }
        let invalid = TransactionSigned::new_unhashed(
            TxEip1559::default().into(),
            Signature::new(U256::ZERO, U256::ZERO, false),
        );
        let result = ingress.recover_raw(invalid.encoded_2718().into()).unwrap();
        assert!(matches!(result.await.unwrap(), Err(IngressError::Recovery(_))));
        assert_eq!(cache.get(invalid.tx_hash()), None);
        assert!(pool.is_empty());
    }

    #[tokio::test]
    async fn prepared_rpc_keeps_its_permit_without_double_admission() {
        let (executor, worker) =
            TransactionValidationTaskExecutor::new(OkValidator::<EthPooledTransaction>::default());
        let worker = tokio::spawn(worker.run());
        let pool = Pool::new(
            executor,
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            PoolConfig {
                ingress: TransactionIngressConfig { max_transactions: 1, ..Default::default() },
                ..Default::default()
            },
        );
        let ingress = pool.transaction_ingress().unwrap();
        let tx = TransactionGenerator::new(rand::rng()).gen_eip1559();
        let hash = *tx.tx_hash();
        let IngressOutcome::Recovered(transaction, permit) =
            ingress.recover_raw(tx.encoded_2718().into()).unwrap().await.unwrap().unwrap()
        else {
            panic!("recovery result")
        };
        assert!(matches!(ingress.reserve(0, 1), Err(IngressError::Full)));
        let result = ingress
            .submit_admitted(TransactionOrigin::Local, transaction, permit)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, IngressOutcome::Inserted(outcome) if outcome.hash == hash));
        drop((ingress, pool));
        tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn closed_worker_releases_admission_and_closes_the_response() {
        let (executor, worker) =
            TransactionValidationTaskExecutor::new(OkValidator::<EthPooledTransaction>::default());
        drop(worker);
        let pool = Pool::new(
            executor,
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            PoolConfig::default(),
        );
        let ingress = pool.transaction_ingress().unwrap();
        let response = ingress.submit_raw(TransactionOrigin::Local, Bytes::new(), || {}).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(2), response).await.unwrap().is_err());
        assert_eq!(
            ingress.lanes[0].slots.available_permits(),
            pool.config().ingress.max_transactions
        );
    }
}
