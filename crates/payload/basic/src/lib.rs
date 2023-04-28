#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth basic payload job generator

use crate::metrics::PayloadBuilderMetrics;
use futures_core::ready;
use futures_util::FutureExt;
use reth_payload_builder::{
    error::PayloadBuilderError, BuiltPayload, PayloadBuilderAttributes, PayloadJob,
    PayloadJobGenerator,
};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    constants::{
        EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, EMPTY_WITHDRAWALS, RETH_CLIENT_VERSION, SLOT_DURATION,
    },
    proofs, Block, BlockNumberOrTag, ChainSpec, Header, IntoRecoveredTransaction, Receipt,
    SealedBlock, Withdrawal, EMPTY_OMMER_ROOT, H256, U256,
};
use reth_provider::{BlockProvider, BlockSource, PostState, StateProviderFactory};
use reth_revm::{
    database::{State, SubState},
    env::tx_env_with_recovered,
    executor::{
        commit_state_changes, increment_account_balance, post_block_withdrawals_balance_increments,
    },
    into_reth_log,
};
use reth_rlp::Encodable;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use revm::{
    db::{CacheDB, DatabaseRef},
    primitives::{BlockEnv, CfgEnv, EVMError, Env, InvalidTransaction, ResultAndState},
};
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
use tracing::{debug, trace};

mod metrics;

/// The [PayloadJobGenerator] that creates [BasicPayloadJob]s.
pub struct BasicPayloadJobGenerator<Client, Pool, Tasks> {
    /// The client that can interact with the chain.
    client: Client,
    /// txpool
    pool: Pool,
    /// How to spawn building tasks
    executor: Tasks,
    /// The configuration for the job generator.
    config: BasicPayloadJobGeneratorConfig,
    /// Restricts how many generator tasks can be executed at once.
    payload_task_guard: PayloadTaskGuard,
    /// The chain spec.
    chain_spec: Arc<ChainSpec>,
}

// === impl BasicPayloadJobGenerator ===

impl<Client, Pool, Tasks> BasicPayloadJobGenerator<Client, Pool, Tasks> {
    /// Creates a new [BasicPayloadJobGenerator] with the given config.
    pub fn new(
        client: Client,
        pool: Pool,
        executor: Tasks,
        config: BasicPayloadJobGeneratorConfig,
        chain_spec: Arc<ChainSpec>,
    ) -> Self {
        Self {
            client,
            pool,
            executor,
            payload_task_guard: PayloadTaskGuard::new(config.max_payload_tasks),
            config,
            chain_spec,
        }
    }
}

// === impl BasicPayloadJobGenerator ===

impl<Client, Pool, Tasks> BasicPayloadJobGenerator<Client, Pool, Tasks> {}

impl<Client, Pool, Tasks> PayloadJobGenerator for BasicPayloadJobGenerator<Client, Pool, Tasks>
where
    Client: StateProviderFactory + BlockProvider + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + Unpin + 'static,
{
    type Job = BasicPayloadJob<Client, Pool, Tasks>;

    fn new_payload_job(
        &self,
        attributes: PayloadBuilderAttributes,
    ) -> Result<Self::Job, PayloadBuilderError> {
        let parent_block = if attributes.parent.is_zero() {
            // use latest block if parent is zero: genesis block
            self.client
                .block(BlockNumberOrTag::Latest.into())?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(attributes.parent))?
                .seal_slow()
        } else {
            let block = self
                .client
                .find_block_by_hash(attributes.parent, BlockSource::Any)?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(attributes.parent))?;

            // we already now the hash, so we can seal it
            block.seal(attributes.parent)
        };

        // configure evm env based on parent block
        let (initialized_cfg, initialized_block_env) =
            attributes.cfg_and_block_env(&self.chain_spec, &parent_block);

        let config = PayloadConfig {
            initialized_block_env,
            initialized_cfg,
            parent_block: Arc::new(parent_block),
            extra_data: self.config.extradata.clone(),
            attributes,
            chain_spec: Arc::clone(&self.chain_spec),
        };

        // create empty

        let until = tokio::time::Instant::now() + self.config.deadline;
        let deadline = Box::pin(tokio::time::sleep_until(until));

        Ok(BasicPayloadJob {
            config,
            client: self.client.clone(),
            pool: self.pool.clone(),
            executor: self.executor.clone(),
            deadline,
            interval: tokio::time::interval(self.config.interval),
            best_payload: None,
            pending_block: None,
            payload_task_guard: self.payload_task_guard.clone(),
            metrics: Default::default(),
        })
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
    /// Data to include in the block's extra data field.
    extradata: Bytes,
    /// Target gas ceiling for mined blocks, defaults to 30_000_000 gas.
    max_gas_limit: u64,
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

    /// Sets the data to include in the block's extra data field.
    ///
    /// Defaults to the current client version: `rlp(RETH_CLIENT_VERSION)`.
    pub fn extradata(mut self, extradata: Bytes) -> Self {
        self.extradata = extradata;
        self
    }

    /// Sets the target gas ceiling for mined blocks.
    ///
    /// Defaults to 30_000_000 gas.
    pub fn max_gas_limit(mut self, max_gas_limit: u64) -> Self {
        self.max_gas_limit = max_gas_limit;
        self
    }
}

impl Default for BasicPayloadJobGeneratorConfig {
    fn default() -> Self {
        let mut extradata = BytesMut::new();
        RETH_CLIENT_VERSION.as_bytes().encode(&mut extradata);
        Self {
            extradata: extradata.freeze(),
            max_gas_limit: 30_000_000,
            interval: Duration::from_secs(1),
            // 12s slot time
            deadline: SLOT_DURATION,
            max_payload_tasks: 3,
        }
    }
}

/// A basic payload job that continuously builds a payload with the best transactions from the pool.
pub struct BasicPayloadJob<Client, Pool, Tasks> {
    /// The configuration for how the payload will be created.
    config: PayloadConfig,
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
    best_payload: Option<Arc<BuiltPayload>>,
    /// Receiver for the block that is currently being built.
    pending_block: Option<PendingPayload>,
    /// Restricts how many generator tasks can be executed at once.
    payload_task_guard: PayloadTaskGuard,
    /// metrics for this type
    metrics: PayloadBuilderMetrics,
}

impl<Client, Pool, Tasks> Future for BasicPayloadJob<Client, Pool, Tasks>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // check if the deadline is reached
        if this.deadline.as_mut().poll(cx).is_ready() {
            trace!("Payload building deadline reached");
            return Poll::Ready(Ok(()))
        }

        // check if the interval is reached
        while this.interval.poll_tick(cx).is_ready() {
            // start a new job if there is no pending block and we haven't reached the deadline
            if this.pending_block.is_none() {
                trace!("spawn new payload build task");
                let (tx, rx) = oneshot::channel();
                let client = this.client.clone();
                let pool = this.pool.clone();
                let cancel = Cancelled::default();
                let _cancel = cancel.clone();
                let guard = this.payload_task_guard.clone();
                let payload_config = this.config.clone();
                let best_payload = this.best_payload.clone();
                this.metrics.inc_initiated_payload_builds();
                this.executor.spawn_blocking(Box::pin(async move {
                    // acquire the permit for executing the task
                    let _permit = guard.0.acquire().await;
                    build_payload(client, pool, payload_config, cancel, best_payload, tx)
                }));
                this.pending_block = Some(PendingPayload { _cancel, payload: rx });
            }
        }

        // poll the pending block
        if let Some(mut fut) = this.pending_block.take() {
            match fut.poll_unpin(cx) {
                Poll::Ready(Ok(outcome)) => {
                    this.interval.reset();
                    match outcome {
                        BuildOutcome::Better(payload) => {
                            trace!("built better payload");
                            let payload = Arc::new(payload);
                            this.best_payload = Some(payload);
                        }
                        BuildOutcome::Aborted { fees } => {
                            trace!(?fees, "skipped payload build of worse block");
                        }
                        BuildOutcome::Cancelled => {
                            unreachable!("the cancel signal never fired")
                        }
                    }
                }
                Poll::Ready(Err(err)) => {
                    // job failed, but we simply try again next interval
                    trace!(?err, "payload build attempt failed");
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

impl<Client, Pool, Tasks> PayloadJob for BasicPayloadJob<Client, Pool, Tasks>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    fn best_payload(&self) -> Result<Arc<BuiltPayload>, PayloadBuilderError> {
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
        build_empty_payload(&self.client, self.config.clone()).map(Arc::new)
    }
}

/// A future that resolves to the result of the block building job.
struct PendingPayload {
    /// The marker to cancel the job on drop
    _cancel: Cancelled,
    /// The channel to send the result to.
    payload: oneshot::Receiver<Result<BuildOutcome, PayloadBuilderError>>,
}

impl Future for PendingPayload {
    type Output = Result<BuildOutcome, PayloadBuilderError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let res = ready!(self.payload.poll_unpin(cx));
        Poll::Ready(res.map_err(Into::into).and_then(|res| res))
    }
}

/// A marker that can be used to cancel a job.
///
/// If dropped, it will set the `cancelled` flag to true.
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

/// Static config for how to build a payload.
#[derive(Clone)]
struct PayloadConfig {
    /// Pre-configured block environment.
    initialized_block_env: BlockEnv,
    /// Configuration for the environment.
    initialized_cfg: CfgEnv,
    /// The parent block.
    parent_block: Arc<SealedBlock>,
    /// Block extra data.
    extra_data: Bytes,
    /// Requested attributes for the payload.
    attributes: PayloadBuilderAttributes,
    /// The chain spec.
    chain_spec: Arc<ChainSpec>,
}

enum BuildOutcome {
    /// Successfully built a better block.
    Better(BuiltPayload),
    /// Aborted payload building because resulted in worse block wrt. fees.
    Aborted { fees: U256 },
    /// Build job was cancelled
    Cancelled,
}

/// Builds the payload and sends the result to the given channel.
fn build_payload<Pool, Client>(
    client: Client,
    pool: Pool,
    config: PayloadConfig,
    cancel: Cancelled,
    best_payload: Option<Arc<BuiltPayload>>,
    to_job: oneshot::Sender<Result<BuildOutcome, PayloadBuilderError>>,
) where
    Client: StateProviderFactory,
    Pool: TransactionPool,
{
    #[inline(always)]
    fn try_build<Pool, Client>(
        client: Client,
        pool: Pool,
        config: PayloadConfig,
        cancel: Cancelled,
        best_payload: Option<Arc<BuiltPayload>>,
    ) -> Result<BuildOutcome, PayloadBuilderError>
    where
        Client: StateProviderFactory,
        Pool: TransactionPool,
    {
        let PayloadConfig {
            initialized_block_env,
            initialized_cfg,
            parent_block,
            extra_data,
            attributes,
            chain_spec,
        } = config;

        debug!(parent_hash=?parent_block.hash, parent_number=parent_block.number, "building new payload");

        let state = client.state_by_block_hash(parent_block.hash)?;
        let mut db = SubState::new(State::new(state));
        let mut post_state = PostState::default();

        let mut cumulative_gas_used = 0;
        let block_gas_limit: u64 = initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX);

        let mut executed_txs = Vec::new();
        let mut best_txs = pool.best_transactions();

        let mut total_fees = U256::ZERO;
        let base_fee = initialized_block_env.basefee.to::<u64>();

        let block_number = initialized_block_env.number.to::<u64>();

        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as invalid
                // which also removes all dependent transaction from the iterator before we can
                // continue
                best_txs.mark_invalid(&pool_tx);
                continue
            }

            // check if the job was cancelled, if so we can exit early
            if cancel.is_cancelled() {
                return Ok(BuildOutcome::Cancelled)
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_recovered_transaction();

            // Configure the environment for the block.
            let env = Env {
                cfg: initialized_cfg.clone(),
                block: initialized_block_env.clone(),
                tx: tx_env_with_recovered(&tx),
            };

            let mut evm = revm::EVM::with_env(env);
            evm.database(&mut db);

            let ResultAndState { result, state } = match evm.transact() {
                Ok(res) => res,
                Err(err) => {
                    match err {
                        EVMError::Transaction(err) => {
                            if matches!(err, InvalidTransaction::NonceTooLow { .. }) {
                                // if the nonce is too low, we can skip this transaction
                                trace!(?err, ?tx, "skipping nonce too low transaction");
                            } else {
                                // if the transaction is invalid, we can skip it and all of its
                                // descendants
                                trace!(
                                    ?err,
                                    ?tx,
                                    "skipping invalid transaction and its descendants"
                                );
                                best_txs.mark_invalid(&pool_tx);
                            }
                            continue
                        }
                        err => {
                            // this is an error that we should treat as fatal for this attempt
                            return Err(PayloadBuilderError::EvmExecutionError(err))
                        }
                    }
                }
            };

            // let ResultAndState { result, state } =
            evm.transact().map_err(PayloadBuilderError::EvmExecutionError)?;
            let gas_used = result.gas_used();

            // commit changes
            commit_state_changes(&mut db, &mut post_state, block_number, state, true);

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            cumulative_gas_used += gas_used;

            // Push transaction changeset and calculate header bloom filter for receipt.
            post_state.add_receipt(Receipt {
                tx_type: tx.tx_type(),
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.logs().into_iter().map(into_reth_log).collect(),
            });

            // update add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(base_fee)
                .expect("fee is always valid; execution succeeded");
            total_fees += U256::from(miner_fee) * U256::from(gas_used);

            // append transaction to the list of executed transactions
            executed_txs.push(tx.into_signed());
        }

        // check if we have a better block
        if !is_better_payload(best_payload.as_deref(), total_fees) {
            // can skip building the block
            return Ok(BuildOutcome::Aborted { fees: total_fees })
        }

        let WithdrawalsOutcome { withdrawals_root, withdrawals } = commit_withdrawals(
            &mut db,
            &mut post_state,
            &chain_spec,
            block_number,
            attributes.timestamp,
            attributes.withdrawals,
        )?;

        let receipts_root = post_state.receipts_root();
        let logs_bloom = post_state.logs_bloom();

        // calculate the state root
        let state_root = db.db.0.state_root(post_state)?;

        // create the block header
        let transactions_root = proofs::calculate_transaction_root(executed_txs.iter());

        let header = Header {
            parent_hash: parent_block.hash,
            ommers_hash: EMPTY_OMMER_ROOT,
            beneficiary: initialized_block_env.coinbase,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root,
            logs_bloom,
            timestamp: attributes.timestamp,
            mix_hash: attributes.prev_randao,
            nonce: 0,
            base_fee_per_gas: Some(base_fee),
            number: parent_block.number + 1,
            gas_limit: block_gas_limit,
            difficulty: U256::ZERO,
            gas_used: cumulative_gas_used,
            extra_data: extra_data.into(),
        };

        // seal the block
        let block = Block { header, body: executed_txs, ommers: vec![], withdrawals };

        let sealed_block = block.seal_slow();
        Ok(BuildOutcome::Better(BuiltPayload::new(attributes.id, sealed_block, total_fees)))
    }
    let _ = to_job.send(try_build(client, pool, config, cancel, best_payload));
}

/// Builds an empty payload without any transactions.
fn build_empty_payload<Client>(
    client: &Client,
    config: PayloadConfig,
) -> Result<BuiltPayload, PayloadBuilderError>
where
    Client: StateProviderFactory,
{
    let PayloadConfig {
        initialized_block_env,
        parent_block,
        extra_data,
        attributes,
        chain_spec,
        ..
    } = config;

    debug!(parent_hash=?parent_block.hash, parent_number=parent_block.number,  "building empty payload");

    let state = client.state_by_block_hash(parent_block.hash)?;
    let mut db = SubState::new(State::new(state));
    let mut post_state = PostState::default();

    let base_fee = initialized_block_env.basefee.to::<u64>();
    let block_number = initialized_block_env.number.to::<u64>();
    let block_gas_limit: u64 = initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX);

    let WithdrawalsOutcome { withdrawals_root, withdrawals } = commit_withdrawals(
        &mut db,
        &mut post_state,
        &chain_spec,
        block_number,
        attributes.timestamp,
        attributes.withdrawals,
    )?;

    // calculate the state root
    let state_root = db.db.0.state_root(post_state)?;

    let header = Header {
        parent_hash: parent_block.hash,
        ommers_hash: EMPTY_OMMER_ROOT,
        beneficiary: initialized_block_env.coinbase,
        state_root,
        transactions_root: EMPTY_TRANSACTIONS,
        withdrawals_root,
        receipts_root: EMPTY_RECEIPTS,
        logs_bloom: Default::default(),
        timestamp: attributes.timestamp,
        mix_hash: attributes.prev_randao,
        nonce: 0,
        base_fee_per_gas: Some(base_fee),
        number: parent_block.number + 1,
        gas_limit: block_gas_limit,
        difficulty: U256::ZERO,
        gas_used: 0,
        extra_data: extra_data.into(),
    };

    let block = Block { header, body: vec![], ommers: vec![], withdrawals };
    let sealed_block = block.seal_slow();

    Ok(BuiltPayload::new(attributes.id, sealed_block, U256::ZERO))
}

/// Represents the outcome of committing withdrawals to the runtime database and post state.
/// Pre-shanghai these are `None` values.
struct WithdrawalsOutcome {
    withdrawals: Option<Vec<Withdrawal>>,
    withdrawals_root: Option<H256>,
}

impl WithdrawalsOutcome {
    /// No withdrawals pre shanghai
    fn pre_shanghai() -> Self {
        Self { withdrawals: None, withdrawals_root: None }
    }

    fn empty() -> Self {
        Self { withdrawals: Some(vec![]), withdrawals_root: Some(EMPTY_WITHDRAWALS) }
    }
}

/// Executes the withdrawals and commits them to the _runtime_ Database and PostState.
///
/// Returns the withdrawals root.
///
/// Returns `None` values pre shanghai
#[allow(clippy::too_many_arguments)]
fn commit_withdrawals<DB>(
    db: &mut CacheDB<DB>,
    post_state: &mut PostState,
    chain_spec: &ChainSpec,
    block_number: u64,
    timestamp: u64,
    withdrawals: Vec<Withdrawal>,
) -> Result<WithdrawalsOutcome, <DB as DatabaseRef>::Error>
where
    DB: DatabaseRef,
{
    if !chain_spec.is_shanghai_activated_at_timestamp(timestamp) {
        return Ok(WithdrawalsOutcome::pre_shanghai())
    }

    if withdrawals.is_empty() {
        return Ok(WithdrawalsOutcome::empty())
    }

    let balance_increments =
        post_block_withdrawals_balance_increments(chain_spec, timestamp, &withdrawals);

    for (address, increment) in balance_increments {
        increment_account_balance(db, post_state, block_number, address, increment)?;
    }

    let withdrawals_root = proofs::calculate_withdrawals_root(withdrawals.iter());

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
fn is_better_payload(best_payload: Option<&BuiltPayload>, new_fees: U256) -> bool {
    if let Some(best_payload) = best_payload {
        new_fees > best_payload.fees()
    } else {
        true
    }
}
