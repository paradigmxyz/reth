#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth basic payload job generator

use futures_core::{ready, Stream};
use futures_util::FutureExt;
use reth_miner::{
    error::PayloadBuilderError, BuiltPayload, PayloadBuilderAttributes, PayloadJob,
    PayloadJobGenerator,
};
use reth_primitives::bytes::Bytes;
use reth_provider::StateProviderFactory;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{
    future::Future,
    pin::Pin,
    sync::{atomic::AtomicBool, Arc},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{oneshot, Semaphore},
    time::{Interval, Sleep},
};
use tracing::trace;

// TODO move to common since commonly used

/// Settings for how to generate a block
#[derive(Debug, Clone)]
pub struct BlockConfig {
    /// Data to include in the block's extra data field.
    extradata: Bytes,
    /// Target gas ceiling for mined blocks, defaults to 30_000_000 gas.
    max_gas_limit: u64,
}

/// The [PayloadJobGenerator] that creates [BasicPayloadJob]s.
#[derive(Debug)]
pub struct BasicPayloadJobGenerator<Client, Pool, Tasks> {
    /// The client that can interact with the chain.
    client: Client,
    /// txpool
    pool: Pool,
    /// How to spawn building tasks
    executor: Tasks,
    /// The configuration for the job generator.
    config: BasicPayloadJobGeneratorConfig,
    /// The configuration for how to create a block.
    block_config: BlockConfig,
    /// Restricts how many generator tasks can be executed at once.
    payload_task_guard: PayloadTaskGuard,
}

// === impl BasicPayloadJobGenerator ===

impl<Client, Pool, Tasks> BasicPayloadJobGenerator<Client, Pool, Tasks> {
    /// Creates a new [BasicPayloadJobGenerator] with the given config.
    pub fn new(
        client: Client,
        pool: Pool,
        executor: Tasks,
        config: BasicPayloadJobGeneratorConfig,
        block_config: BlockConfig,
    ) -> Self {
        Self {
            client,
            pool,
            executor,
            payload_task_guard: PayloadTaskGuard::new(config.max_payload_tasks),
            config,
            block_config,
        }
    }
}

impl<Client, Pool, Tasks> PayloadJobGenerator for BasicPayloadJob<Client, Pool, Tasks>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + Unpin + 'static,
{
    type Job = BasicPayloadJob<Client, Pool, Tasks>;

    fn new_payload_job(&self, _attr: PayloadBuilderAttributes) -> Self::Job {
        todo!()
    }
}

/// Restricts how many generator tasks can be executed at once.
#[derive(Clone)]
struct PayloadTaskGuard(Arc<Semaphore>);

// === impl PayloadTaskGuard ===

impl PayloadTaskGuard {
    fn new(max_payload_tasks: usize) -> Self {
        Self(Arc::new(Semaphore::new(max_payload_tasks)))
    }
}

/// Settings for the [BasicPayloadJobGenerator].
#[derive(Debug, Clone)]
pub struct BasicPayloadJobGeneratorConfig {
    /// The interval at which the job should build a new payload after the last.
    interval: Duration,
    /// The deadline when this job should resolve.
    deadline: Duration,
    /// Maximum number of tasks to spawn for building a payload.
    max_payload_tasks: usize,
}

// === impl BasicPayloadJobGeneratorConfig ===

impl BasicPayloadJobGeneratorConfig {
    /// Sets the interval at which the job should build a new payload after the last.
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Sets the deadline when this job should resolve.
    pub fn deadline(mut self, deadline: Duration) -> Self {
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
}

impl Default for BasicPayloadJobGeneratorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            // 12s slot time
            deadline: Duration::from_secs(12),
            max_payload_tasks: 3,
        }
    }
}

/// A basic payload job that continuously builds a payload with the best transactions from the pool.
pub struct BasicPayloadJob<Client, Pool, Tasks> {
    /// Requested attributes for the payload.
    attributes: PayloadBuilderAttributes,
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
    best_payload: Arc<BuiltPayload>,
    /// Receiver for the block that is currently being built.
    pending_block: Option<PendingPayload>,
    /// Restricts how many generator tasks can be executed at once.
    payload_task_guard: PayloadTaskGuard,
    /// Settings for how to generate a block.
    block_config: BlockConfig,
}

impl<Client, Pool, Tasks> Stream for BasicPayloadJob<Client, Pool, Tasks>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    type Item = Result<Arc<BuiltPayload>, PayloadBuilderError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // check if the deadline is reached
        let deadline_reached = this.deadline.as_mut().poll(cx).is_ready();

        // check if the interval is reached
        if this.interval.poll_tick(cx).is_ready() &&
            this.pending_block.is_none() &&
            !deadline_reached
        {
            trace!("spawn new payload build task");
            let (tx, rx) = oneshot::channel();
            let client = this.client.clone();
            let pool = this.pool.clone();
            let cancel = Cancelled::default();
            let _cancel = cancel.clone();
            let guard = this.payload_task_guard.clone();
            this.executor.spawn_blocking(Box::pin(async move {
                // acquire the permit for executing the task
                let _permit = guard.0.acquire().await;
                build_payload(client, pool, cancel, tx)
            }));
            this.pending_block = Some(PendingPayload { _cancel, payload: rx });
        }

        // poll the pending block
        if let Some(mut fut) = this.pending_block.take() {
            match fut.poll_unpin(cx) {
                Poll::Ready(Ok(payload)) => {
                    this.interval.reset();
                    // TODO check if payload is better
                    let payload = Arc::new(payload);
                    this.best_payload = payload.clone();
                    return Poll::Ready(Some(Ok(payload)))
                }
                Poll::Ready(Err(err)) => {
                    this.interval.reset();
                    return Poll::Ready(Some(Err(err)))
                }
                Poll::Pending => {
                    this.pending_block = Some(fut);
                }
            }
        }

        if deadline_reached {
            trace!("Payload building deadline reached");
            return Poll::Ready(None)
        }

        Poll::Pending
    }
}

impl<Client, Pool, Tasks> PayloadJob for BasicPayloadJob<Client, Pool, Tasks>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    fn best_payload(&self) -> Arc<BuiltPayload> {
        self.best_payload.clone()
    }
}

/// A future that resolves to the result of the block building job.
struct PendingPayload {
    /// The marker to cancel the job on drop
    _cancel: Cancelled,
    /// The channel to send the result to.
    payload: oneshot::Receiver<Result<BuiltPayload, PayloadBuilderError>>,
}

impl Future for PendingPayload {
    type Output = Result<BuiltPayload, PayloadBuilderError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let res = ready!(self.payload.poll_unpin(cx));
        Poll::Ready(res.map_err(Into::into).and_then(|res| res))
    }
}

/// A marker that can be used to cancel a job.
#[derive(Default, Clone)]
struct Cancelled(Arc<AtomicBool>);

// === impl Cancelled ===

impl Cancelled {
    /// Returns true if the job was cancelled.
    fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for Cancelled {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Builds the payload and sends the result to the given channel.
fn build_payload<Pool, Client>(
    _client: Client,
    _pool: Pool,
    _cancel: Cancelled,
    to_job: oneshot::Sender<Result<BuiltPayload, PayloadBuilderError>>,
) where
    Client: StateProviderFactory,
    Pool: TransactionPool,
{
    #[inline(always)]
    fn try_build<Pool, Client>(
        _client: Client,
        _pool: Pool,
        _cancel: Cancelled,
    ) -> Result<BuiltPayload, PayloadBuilderError>
    where
        Client: StateProviderFactory,
        Pool: TransactionPool,
    {
        todo!()
    }
    let _ = to_job.send(try_build(_client, _pool, _cancel));
}
