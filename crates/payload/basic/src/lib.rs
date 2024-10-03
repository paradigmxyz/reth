//! A basic payload generator for reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use crate::metrics::PayloadBuilderMetrics;
use alloy_primitives::{Bytes, B256, U256};
use futures_core::ready;
use futures_util::FutureExt;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_payload_builder::{
    database::CachedReads, KeepPayloadJobAlive, PayloadId, PayloadJob, PayloadJobGenerator,
};
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes, PayloadBuilderError};
use reth_primitives::{
    constants::{EMPTY_WITHDRAWALS, RETH_CLIENT_VERSION, SLOT_DURATION},
    proofs, BlockNumberOrTag, SealedBlock, Withdrawals,
};
use reth_provider::{
    BlockReaderIdExt, BlockSource, CanonStateNotification, ProviderError, StateProviderFactory,
};
use reth_revm::state_change::post_block_withdrawals_balance_increments;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use revm::{Database, State};
use std::{
    fmt,
    future::Future,
    ops::Deref,
    pin::Pin,
    sync::{atomic::AtomicBool, Arc},
    task::{Context, Poll},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{oneshot, Semaphore},
    time::{Interval, Sleep},
};
use tracing::{debug, trace, warn};

mod metrics;

/// The [`PayloadJobGenerator`] that creates [`BasicPayloadJob`]s.
#[derive(Debug)]
pub struct BasicPayloadJobGenerator<Client, Pool, Tasks, Builder> {
    /// The client that can interact with the chain.
    client: Client,
    /// The transaction pool to pull transactions from.
    pool: Pool,
    /// The task executor to spawn payload building tasks on.
    executor: Tasks,
    /// The configuration for the job generator.
    config: BasicPayloadJobGeneratorConfig,
    /// Restricts how many generator tasks can be executed at once.
    payload_task_guard: PayloadTaskGuard,
    /// The type responsible for building payloads.
    ///
    /// See [`PayloadBuilder`]
    builder: Builder,
    /// Stored `cached_reads` for new payload jobs.
    pre_cached: Option<PrecachedState>,
}

// === impl BasicPayloadJobGenerator ===

impl<Client, Pool, Tasks, Builder> BasicPayloadJobGenerator<Client, Pool, Tasks, Builder> {
    /// Creates a new [`BasicPayloadJobGenerator`] with the given config and custom
    /// [`PayloadBuilder`]
    pub fn with_builder(
        client: Client,
        pool: Pool,
        executor: Tasks,
        config: BasicPayloadJobGeneratorConfig,
        builder: Builder,
    ) -> Self {
        Self {
            client,
            pool,
            executor,
            payload_task_guard: PayloadTaskGuard::new(config.max_payload_tasks),
            config,
            builder,
            pre_cached: None,
        }
    }

    /// Returns the maximum duration a job should be allowed to run.
    ///
    /// This adheres to the following specification:
    // > Client software SHOULD stop the updating process when either a call to engine_getPayload
    // > with the build process's payloadId is made or SECONDS_PER_SLOT (12s in the Mainnet
    // > configuration) have passed since the point in time identified by the timestamp parameter.
    // See also <https://github.com/ethereum/execution-apis/blob/431cf72fd3403d946ca3e3afc36b973fc87e0e89/src/engine/paris.md?plain=1#L137>
    #[inline]
    fn max_job_duration(&self, unix_timestamp: u64) -> Duration {
        let duration_until_timestamp = duration_until(unix_timestamp);

        // safety in case clocks are bad
        let duration_until_timestamp = duration_until_timestamp.min(self.config.deadline * 3);

        self.config.deadline + duration_until_timestamp
    }

    /// Returns the [Instant](tokio::time::Instant) at which the job should be terminated because it
    /// is considered timed out.
    #[inline]
    fn job_deadline(&self, unix_timestamp: u64) -> tokio::time::Instant {
        tokio::time::Instant::now() + self.max_job_duration(unix_timestamp)
    }

    /// Returns a reference to the tasks type
    pub const fn tasks(&self) -> &Tasks {
        &self.executor
    }

    /// Returns the pre-cached reads for the given parent block if it matches the cached state's
    /// block.
    fn maybe_pre_cached(&self, parent: B256) -> Option<CachedReads> {
        self.pre_cached.as_ref().filter(|pc| pc.block == parent).map(|pc| pc.cached.clone())
    }
}

// === impl BasicPayloadJobGenerator ===

impl<Client, Pool, Tasks, Builder> PayloadJobGenerator
    for BasicPayloadJobGenerator<Client, Pool, Tasks, Builder>
where
    Client: StateProviderFactory + BlockReaderIdExt + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + Unpin + 'static,
    Builder: PayloadBuilder<Pool, Client> + Unpin + 'static,
    <Builder as PayloadBuilder<Pool, Client>>::Attributes: Unpin + Clone,
    <Builder as PayloadBuilder<Pool, Client>>::BuiltPayload: Unpin + Clone,
{
    type Job = BasicPayloadJob<Client, Pool, Tasks, Builder>;

    fn new_payload_job(
        &self,
        attributes: <Self::Job as PayloadJob>::PayloadAttributes,
    ) -> Result<Self::Job, PayloadBuilderError> {
        let parent_block = if attributes.parent().is_zero() {
            // use latest block if parent is zero: genesis block
            self.client
                .block_by_number_or_tag(BlockNumberOrTag::Latest)?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(attributes.parent()))?
                .seal_slow()
        } else {
            let block = self
                .client
                .find_block_by_hash(attributes.parent(), BlockSource::Any)?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(attributes.parent()))?;

            // we already know the hash, so we can seal it
            block.seal(attributes.parent())
        };

        let config =
            PayloadConfig::new(Arc::new(parent_block), self.config.extradata.clone(), attributes);

        let until = self.job_deadline(config.attributes.timestamp());
        let deadline = Box::pin(tokio::time::sleep_until(until));

        let cached_reads = self.maybe_pre_cached(config.parent_block.hash());

        let mut job = BasicPayloadJob {
            config,
            client: self.client.clone(),
            pool: self.pool.clone(),
            executor: self.executor.clone(),
            deadline,
            // ticks immediately
            interval: tokio::time::interval(self.config.interval),
            best_payload: None,
            pending_block: None,
            cached_reads,
            payload_task_guard: self.payload_task_guard.clone(),
            metrics: Default::default(),
            builder: self.builder.clone(),
        };

        // start the first job right away
        job.spawn_build_job();

        Ok(job)
    }

    fn on_new_state(&mut self, new_state: CanonStateNotification) {
        let mut cached = CachedReads::default();

        // extract the state from the notification and put it into the cache
        let committed = new_state.committed();
        let new_execution_outcome = committed.execution_outcome();
        for (addr, acc) in new_execution_outcome.bundle_accounts_iter() {
            if let Some(info) = acc.info.clone() {
                // we want pre cache existing accounts and their storage
                // this only includes changed accounts and storage but is better than nothing
                let storage =
                    acc.storage.iter().map(|(key, slot)| (*key, slot.present_value)).collect();
                cached.insert_account(addr, info, storage);
            }
        }

        self.pre_cached = Some(PrecachedState { block: committed.tip().hash(), cached });
    }
}

/// Pre-filled [`CachedReads`] for a specific block.
///
/// This is extracted from the [`CanonStateNotification`] for the tip block.
#[derive(Debug, Clone)]
pub struct PrecachedState {
    /// The block for which the state is pre-cached.
    pub block: B256,
    /// Cached state for the block.
    pub cached: CachedReads,
}

/// Restricts how many generator tasks can be executed at once.
#[derive(Debug, Clone)]
pub struct PayloadTaskGuard(Arc<Semaphore>);

impl Deref for PayloadTaskGuard {
    type Target = Semaphore;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// === impl PayloadTaskGuard ===

impl PayloadTaskGuard {
    /// Constructs `Self` with a maximum task count of `max_payload_tasks`.
    pub fn new(max_payload_tasks: usize) -> Self {
        Self(Arc::new(Semaphore::new(max_payload_tasks)))
    }
}

/// Settings for the [`BasicPayloadJobGenerator`].
#[derive(Debug, Clone)]
pub struct BasicPayloadJobGeneratorConfig {
    /// Data to include in the block's extra data field.
    extradata: Bytes,
    /// The interval at which the job should build a new payload after the last.
    interval: Duration,
    /// The deadline for when the payload builder job should resolve.
    ///
    /// By default this is [`SLOT_DURATION`]: 12s
    deadline: Duration,
    /// Maximum number of tasks to spawn for building a payload.
    max_payload_tasks: usize,
}

// === impl BasicPayloadJobGeneratorConfig ===

impl BasicPayloadJobGeneratorConfig {
    /// Sets the interval at which the job should build a new payload after the last.
    pub const fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Sets the deadline when this job should resolve.
    pub const fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Sets the maximum number of tasks to spawn for building a payload(s).
    ///
    /// # Panics
    ///
    /// If `max_payload_tasks` is 0.
    pub fn max_payload_tasks(mut self, max_payload_tasks: usize) -> Self {
        assert!(max_payload_tasks > 0, "max_payload_tasks must be greater than 0");
        self.max_payload_tasks = max_payload_tasks;
        self
    }

    /// Sets the data to include in the block's extra data field.
    ///
    /// Defaults to the current client version: `rlp(RETH_CLIENT_VERSION)`.
    pub fn extradata(mut self, extradata: Bytes) -> Self {
        self.extradata = extradata;
        self
    }
}

impl Default for BasicPayloadJobGeneratorConfig {
    fn default() -> Self {
        Self {
            extradata: alloy_rlp::encode(RETH_CLIENT_VERSION.as_bytes()).into(),
            interval: Duration::from_secs(1),
            // 12s slot time
            deadline: SLOT_DURATION,
            max_payload_tasks: 3,
        }
    }
}

/// A basic payload job that continuously builds a payload with the best transactions from the pool.
#[derive(Debug)]
pub struct BasicPayloadJob<Client, Pool, Tasks, Builder>
where
    Builder: PayloadBuilder<Pool, Client>,
{
    /// The configuration for how the payload will be created.
    config: PayloadConfig<Builder::Attributes>,
    /// The client that can interact with the chain.
    client: Client,
    /// The transaction pool.
    pool: Pool,
    /// How to spawn building tasks
    executor: Tasks,
    /// The deadline when this job should resolve.
    deadline: Pin<Box<Sleep>>,
    /// The interval at which the job should build a new payload after the last.
    interval: Interval,
    /// The best payload so far.
    best_payload: Option<Builder::BuiltPayload>,
    /// Receiver for the block that is currently being built.
    pending_block: Option<PendingPayload<Builder::BuiltPayload>>,
    /// Restricts how many generator tasks can be executed at once.
    payload_task_guard: PayloadTaskGuard,
    /// Caches all disk reads for the state the new payloads builds on
    ///
    /// This is used to avoid reading the same state over and over again when new attempts are
    /// triggered, because during the building process we'll repeatedly execute the transactions.
    cached_reads: Option<CachedReads>,
    /// metrics for this type
    metrics: PayloadBuilderMetrics,
    /// The type responsible for building payloads.
    ///
    /// See [`PayloadBuilder`]
    builder: Builder,
}

impl<Client, Pool, Tasks, Builder> BasicPayloadJob<Client, Pool, Tasks, Builder>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Builder: PayloadBuilder<Pool, Client> + Unpin + 'static,
    <Builder as PayloadBuilder<Pool, Client>>::Attributes: Unpin + Clone,
    <Builder as PayloadBuilder<Pool, Client>>::BuiltPayload: Unpin + Clone,
{
    /// Spawns a new payload build task.
    fn spawn_build_job(&mut self) {
        trace!(target: "payload_builder", id = %self.config.payload_id(), "spawn new payload build task");
        let (tx, rx) = oneshot::channel();
        let client = self.client.clone();
        let pool = self.pool.clone();
        let cancel = Cancelled::default();
        let _cancel = cancel.clone();
        let guard = self.payload_task_guard.clone();
        let payload_config = self.config.clone();
        let best_payload = self.best_payload.clone();
        self.metrics.inc_initiated_payload_builds();
        let cached_reads = self.cached_reads.take().unwrap_or_default();
        let builder = self.builder.clone();
        self.executor.spawn_blocking(Box::pin(async move {
            // acquire the permit for executing the task
            let _permit = guard.acquire().await;
            let args = BuildArguments {
                client,
                pool,
                cached_reads,
                config: payload_config,
                cancel,
                best_payload,
            };
            let result = builder.try_build(args);
            let _ = tx.send(result);
        }));

        self.pending_block = Some(PendingPayload { _cancel, payload: rx });
    }
}

impl<Client, Pool, Tasks, Builder> Future for BasicPayloadJob<Client, Pool, Tasks, Builder>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Builder: PayloadBuilder<Pool, Client> + Unpin + 'static,
    <Builder as PayloadBuilder<Pool, Client>>::Attributes: Unpin + Clone,
    <Builder as PayloadBuilder<Pool, Client>>::BuiltPayload: Unpin + Clone,
{
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // check if the deadline is reached
        if this.deadline.as_mut().poll(cx).is_ready() {
            trace!(target: "payload_builder", "payload building deadline reached");
            return Poll::Ready(Ok(()))
        }

        // check if the interval is reached
        while this.interval.poll_tick(cx).is_ready() {
            // start a new job if there is no pending block and we haven't reached the deadline
            if this.pending_block.is_none() {
                this.spawn_build_job();
            }
        }

        // poll the pending block
        if let Some(mut fut) = this.pending_block.take() {
            match fut.poll_unpin(cx) {
                Poll::Ready(Ok(outcome)) => match outcome {
                    BuildOutcome::Better { payload, cached_reads } => {
                        this.cached_reads = Some(cached_reads);
                        debug!(target: "payload_builder", value = %payload.fees(), "built better payload");
                        this.best_payload = Some(payload);
                    }
                    BuildOutcome::Aborted { fees, cached_reads } => {
                        this.cached_reads = Some(cached_reads);
                        trace!(target: "payload_builder", worse_fees = %fees, "skipped payload build of worse block");
                    }
                    BuildOutcome::Cancelled => {
                        unreachable!("the cancel signal never fired")
                    }
                },
                Poll::Ready(Err(error)) => {
                    // job failed, but we simply try again next interval
                    debug!(target: "payload_builder", %error, "payload build attempt failed");
                    this.metrics.inc_failed_payload_builds();
                }
                Poll::Pending => {
                    this.pending_block = Some(fut);
                }
            }
        }

        Poll::Pending
    }
}

impl<Client, Pool, Tasks, Builder> PayloadJob for BasicPayloadJob<Client, Pool, Tasks, Builder>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Builder: PayloadBuilder<Pool, Client> + Unpin + 'static,
    <Builder as PayloadBuilder<Pool, Client>>::Attributes: Unpin + Clone,
    <Builder as PayloadBuilder<Pool, Client>>::BuiltPayload: Unpin + Clone,
{
    type PayloadAttributes = Builder::Attributes;
    type ResolvePayloadFuture = ResolveBestPayload<Self::BuiltPayload>;
    type BuiltPayload = Builder::BuiltPayload;

    fn best_payload(&self) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        if let Some(ref payload) = self.best_payload {
            return Ok(payload.clone())
        }
        // No payload has been built yet, but we need to return something that the CL then can
        // deliver, so we need to return an empty payload.
        //
        // Note: it is assumed that this is unlikely to happen, as the payload job is started right
        // away and the first full block should have been built by the time CL is requesting the
        // payload.
        self.metrics.inc_requested_empty_payload();
        self.builder.build_empty_payload(&self.client, self.config.clone())
    }

    fn payload_attributes(&self) -> Result<Self::PayloadAttributes, PayloadBuilderError> {
        Ok(self.config.attributes.clone())
    }

    fn resolve(&mut self) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let best_payload = self.best_payload.take();

        if best_payload.is_none() && self.pending_block.is_none() {
            // ensure we have a job scheduled if we don't have a best payload yet and none is active
            self.spawn_build_job();
        }

        let maybe_better = self.pending_block.take();
        let mut empty_payload = None;

        if best_payload.is_none() {
            debug!(target: "payload_builder", id=%self.config.payload_id(), "no best payload yet to resolve, building empty payload");

            let args = BuildArguments {
                client: self.client.clone(),
                pool: self.pool.clone(),
                cached_reads: self.cached_reads.take().unwrap_or_default(),
                config: self.config.clone(),
                cancel: Cancelled::default(),
                best_payload: None,
            };

            match self.builder.on_missing_payload(args) {
                MissingPayloadBehaviour::AwaitInProgress => {
                    debug!(target: "payload_builder", id=%self.config.payload_id(), "awaiting in progress payload build job");
                }
                MissingPayloadBehaviour::RaceEmptyPayload => {
                    debug!(target: "payload_builder", id=%self.config.payload_id(), "racing empty payload");

                    // if no payload has been built yet
                    self.metrics.inc_requested_empty_payload();
                    // no payload built yet, so we need to return an empty payload
                    let (tx, rx) = oneshot::channel();
                    let client = self.client.clone();
                    let config = self.config.clone();
                    let builder = self.builder.clone();
                    self.executor.spawn_blocking(Box::pin(async move {
                        let res = builder.build_empty_payload(&client, config);
                        let _ = tx.send(res);
                    }));

                    empty_payload = Some(rx);
                }
                MissingPayloadBehaviour::RacePayload(job) => {
                    debug!(target: "payload_builder", id=%self.config.payload_id(), "racing fallback payload");
                    // race the in progress job with this job
                    let (tx, rx) = oneshot::channel();
                    self.executor.spawn_blocking(Box::pin(async move {
                        let _ = tx.send(job());
                    }));
                    empty_payload = Some(rx);
                }
            };
        }

        let fut = ResolveBestPayload { best_payload, maybe_better, empty_payload };

        (fut, KeepPayloadJobAlive::No)
    }
}

/// The future that returns the best payload to be served to the consensus layer.
///
/// This returns the payload that's supposed to be sent to the CL.
///
/// If payload has been built so far, it will return that, but it will check if there's a better
/// payload available from an in progress build job. If so it will return that.
///
/// If no payload has been built so far, it will either return an empty payload or the result of the
/// in progress build job, whatever finishes first.
#[derive(Debug)]
pub struct ResolveBestPayload<Payload> {
    /// Best payload so far.
    pub best_payload: Option<Payload>,
    /// Regular payload job that's currently running that might produce a better payload.
    pub maybe_better: Option<PendingPayload<Payload>>,
    /// The empty payload building job in progress, if any.
    pub empty_payload: Option<oneshot::Receiver<Result<Payload, PayloadBuilderError>>>,
}

impl<Payload> ResolveBestPayload<Payload> {
    const fn is_empty(&self) -> bool {
        self.best_payload.is_none() && self.maybe_better.is_none() && self.empty_payload.is_none()
    }
}

impl<Payload> Future for ResolveBestPayload<Payload>
where
    Payload: Unpin,
{
    type Output = Result<Payload, PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // check if there is a better payload before returning the best payload
        if let Some(fut) = Pin::new(&mut this.maybe_better).as_pin_mut() {
            if let Poll::Ready(res) = fut.poll(cx) {
                this.maybe_better = None;
                if let Ok(BuildOutcome::Better { payload, .. }) = res {
                    debug!(target: "payload_builder", "resolving better payload");
                    return Poll::Ready(Ok(payload))
                }
            }
        }

        if let Some(best) = this.best_payload.take() {
            debug!(target: "payload_builder", "resolving best payload");
            return Poll::Ready(Ok(best))
        }

        if let Some(fut) = Pin::new(&mut this.empty_payload).as_pin_mut() {
            if let Poll::Ready(res) = fut.poll(cx) {
                this.empty_payload = None;
                return match res {
                    Ok(res) => {
                        if let Err(err) = &res {
                            warn!(target: "payload_builder", %err, "failed to resolve empty payload");
                        } else {
                            debug!(target: "payload_builder", "resolving empty payload");
                        }
                        Poll::Ready(res)
                    }
                    Err(err) => Poll::Ready(Err(err.into())),
                }
            }
        }

        if this.is_empty() {
            return Poll::Ready(Err(PayloadBuilderError::MissingPayload))
        }

        Poll::Pending
    }
}

/// A future that resolves to the result of the block building job.
#[derive(Debug)]
pub struct PendingPayload<P> {
    /// The marker to cancel the job on drop
    _cancel: Cancelled,
    /// The channel to send the result to.
    payload: oneshot::Receiver<Result<BuildOutcome<P>, PayloadBuilderError>>,
}

impl<P> PendingPayload<P> {
    /// Constructs a `PendingPayload` future.
    pub const fn new(
        cancel: Cancelled,
        payload: oneshot::Receiver<Result<BuildOutcome<P>, PayloadBuilderError>>,
    ) -> Self {
        Self { _cancel: cancel, payload }
    }
}

impl<P> Future for PendingPayload<P> {
    type Output = Result<BuildOutcome<P>, PayloadBuilderError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let res = ready!(self.payload.poll_unpin(cx));
        Poll::Ready(res.map_err(Into::into).and_then(|res| res))
    }
}

/// A marker that can be used to cancel a job.
///
/// If dropped, it will set the `cancelled` flag to true.
#[derive(Default, Clone, Debug)]
pub struct Cancelled(Arc<AtomicBool>);

// === impl Cancelled ===

impl Cancelled {
    /// Returns true if the job was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for Cancelled {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Static config for how to build a payload.
#[derive(Clone, Debug)]
pub struct PayloadConfig<Attributes> {
    /// The parent block.
    pub parent_block: Arc<SealedBlock>,
    /// Block extra data.
    pub extra_data: Bytes,
    /// Requested attributes for the payload.
    pub attributes: Attributes,
}

impl<Attributes> PayloadConfig<Attributes> {
    /// Returns an owned instance of the [`PayloadConfig`]'s `extra_data` bytes.
    pub fn extra_data(&self) -> Bytes {
        self.extra_data.clone()
    }
}

impl<Attributes> PayloadConfig<Attributes>
where
    Attributes: PayloadBuilderAttributes,
{
    /// Create new payload config.
    pub const fn new(
        parent_block: Arc<SealedBlock>,
        extra_data: Bytes,
        attributes: Attributes,
    ) -> Self {
        Self { parent_block, extra_data, attributes }
    }

    /// Returns the payload id.
    pub fn payload_id(&self) -> PayloadId {
        self.attributes.payload_id()
    }
}

/// The possible outcomes of a payload building attempt.
#[derive(Debug)]
pub enum BuildOutcome<Payload> {
    /// Successfully built a better block.
    Better {
        /// The new payload that was built.
        payload: Payload,
        /// The cached reads that were used to build the payload.
        cached_reads: CachedReads,
    },
    /// Aborted payload building because resulted in worse block wrt. fees.
    Aborted {
        /// The total fees associated with the attempted payload.
        fees: U256,
        /// The cached reads that were used to build the payload.
        cached_reads: CachedReads,
    },
    /// Build job was cancelled
    Cancelled,
}

impl<Payload> BuildOutcome<Payload> {
    /// Consumes the type and returns the payload if the outcome is `Better`.
    pub fn into_payload(self) -> Option<Payload> {
        match self {
            Self::Better { payload, .. } => Some(payload),
            _ => None,
        }
    }

    /// Returns true if the outcome is `Better`.
    pub const fn is_better(&self) -> bool {
        matches!(self, Self::Better { .. })
    }

    /// Returns true if the outcome is `Aborted`.
    pub const fn is_aborted(&self) -> bool {
        matches!(self, Self::Aborted { .. })
    }

    /// Returns true if the outcome is `Cancelled`.
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// A collection of arguments used for building payloads.
///
/// This struct encapsulates the essential components and configuration required for the payload
/// building process. It holds references to the Ethereum client, transaction pool, cached reads,
/// payload configuration, cancellation status, and the best payload achieved so far.
#[derive(Debug)]
pub struct BuildArguments<Pool, Client, Attributes, Payload> {
    /// How to interact with the chain.
    pub client: Client,
    /// The transaction pool.
    ///
    /// Or the type that provides the transactions to build the payload.
    pub pool: Pool,
    /// Previously cached disk reads
    pub cached_reads: CachedReads,
    /// How to configure the payload.
    pub config: PayloadConfig<Attributes>,
    /// A marker that can be used to cancel the job.
    pub cancel: Cancelled,
    /// The best payload achieved so far.
    pub best_payload: Option<Payload>,
}

impl<Pool, Client, Attributes, Payload> BuildArguments<Pool, Client, Attributes, Payload> {
    /// Create new build arguments.
    pub const fn new(
        client: Client,
        pool: Pool,
        cached_reads: CachedReads,
        config: PayloadConfig<Attributes>,
        cancel: Cancelled,
        best_payload: Option<Payload>,
    ) -> Self {
        Self { client, pool, cached_reads, config, cancel, best_payload }
    }

    /// Maps the transaction pool to a new type.
    pub fn with_pool<P>(self, pool: P) -> BuildArguments<P, Client, Attributes, Payload> {
        BuildArguments {
            client: self.client,
            pool,
            cached_reads: self.cached_reads,
            config: self.config,
            cancel: self.cancel,
            best_payload: self.best_payload,
        }
    }
}

/// A trait for building payloads that encapsulate Ethereum transactions.
///
/// This trait provides the `try_build` method to construct a transaction payload
/// using `BuildArguments`. It returns a `Result` indicating success or a
/// `PayloadBuilderError` if building fails.
///
/// Generic parameters `Pool` and `Client` represent the transaction pool and
/// Ethereum client types.
pub trait PayloadBuilder<Pool, Client>: Send + Sync + Clone {
    /// The payload attributes type to accept for building.
    type Attributes: PayloadBuilderAttributes;
    /// The type of the built payload.
    type BuiltPayload: BuiltPayload;

    /// Tries to build a transaction payload using provided arguments.
    ///
    /// Constructs a transaction payload based on the given arguments,
    /// returning a `Result` indicating success or an error if building fails.
    ///
    /// # Arguments
    ///
    /// - `args`: Build arguments containing necessary components.
    ///
    /// # Returns
    ///
    /// A `Result` indicating the build outcome or an error.
    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError>;

    /// Invoked when the payload job is being resolved and there is no payload yet.
    ///
    /// This can happen if the CL requests a payload before the first payload has been built.
    fn on_missing_payload(
        &self,
        _args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        MissingPayloadBehaviour::RaceEmptyPayload
    }

    /// Builds an empty payload without any transaction.
    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError>;
}

/// Tells the payload builder how to react to payload request if there's no payload available yet.
///
/// This situation can occur if the CL requests a payload before the first payload has been built.
pub enum MissingPayloadBehaviour<Payload> {
    /// Await the regular scheduled payload process.
    AwaitInProgress,
    /// Race the in progress payload process with an empty payload.
    RaceEmptyPayload,
    /// Race the in progress payload process with this job.
    RacePayload(Box<dyn FnOnce() -> Result<Payload, PayloadBuilderError> + Send>),
}

impl<Payload> fmt::Debug for MissingPayloadBehaviour<Payload> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AwaitInProgress => write!(f, "AwaitInProgress"),
            Self::RaceEmptyPayload => {
                write!(f, "RaceEmptyPayload")
            }
            Self::RacePayload(_) => write!(f, "RacePayload"),
        }
    }
}

impl<Payload> Default for MissingPayloadBehaviour<Payload> {
    fn default() -> Self {
        Self::RaceEmptyPayload
    }
}

/// Represents the outcome of committing withdrawals to the runtime database and post state.
/// Pre-shanghai these are `None` values.
#[derive(Default, Debug)]
pub struct WithdrawalsOutcome {
    /// committed withdrawals, if any.
    pub withdrawals: Option<Withdrawals>,
    /// withdrawals root if any.
    pub withdrawals_root: Option<B256>,
}

impl WithdrawalsOutcome {
    /// No withdrawals pre shanghai
    pub const fn pre_shanghai() -> Self {
        Self { withdrawals: None, withdrawals_root: None }
    }

    /// No withdrawals
    pub fn empty() -> Self {
        Self {
            withdrawals: Some(Withdrawals::default()),
            withdrawals_root: Some(EMPTY_WITHDRAWALS),
        }
    }
}

/// Executes the withdrawals and commits them to the _runtime_ Database and `BundleState`.
///
/// Returns the withdrawals root.
///
/// Returns `None` values pre shanghai
pub fn commit_withdrawals<DB: Database<Error = ProviderError>>(
    db: &mut State<DB>,
    chain_spec: &ChainSpec,
    timestamp: u64,
    withdrawals: Withdrawals,
) -> Result<WithdrawalsOutcome, DB::Error> {
    if !chain_spec.is_shanghai_active_at_timestamp(timestamp) {
        return Ok(WithdrawalsOutcome::pre_shanghai())
    }

    if withdrawals.is_empty() {
        return Ok(WithdrawalsOutcome::empty())
    }

    let balance_increments =
        post_block_withdrawals_balance_increments(chain_spec, timestamp, &withdrawals);

    db.increment_balances(balance_increments)?;

    let withdrawals_root = proofs::calculate_withdrawals_root(&withdrawals);

    // calculate withdrawals root
    Ok(WithdrawalsOutcome {
        withdrawals: Some(withdrawals),
        withdrawals_root: Some(withdrawals_root),
    })
}

/// Checks if the new payload is better than the current best.
///
/// This compares the total fees of the blocks, higher is better.
#[inline(always)]
pub fn is_better_payload(best_payload: Option<impl BuiltPayload>, new_fees: U256) -> bool {
    if let Some(best_payload) = best_payload {
        new_fees > best_payload.fees()
    } else {
        true
    }
}

/// Returns the duration until the given unix timestamp in seconds.
///
/// Returns `Duration::ZERO` if the given timestamp is in the past.
fn duration_until(unix_timestamp_secs: u64) -> Duration {
    let unix_now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let timestamp = Duration::from_secs(unix_timestamp_secs);
    timestamp.saturating_sub(unix_now)
}
