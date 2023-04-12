#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
// TODO rm later
#![allow(unused)]

//! reth basic payload job generator

use futures_core::{ready, Stream};
use futures_util::FutureExt;
use reth_consensus_common::validation::calculate_next_block_base_fee;
use reth_miner::{
    error::PayloadBuilderError, BuiltPayload, PayloadBuilderAttributes, PayloadJob,
    PayloadJobGenerator,
};
use reth_primitives::{
    bloom::logs_bloom, bytes::Bytes, proofs, Block, ChainSpec, Hardfork, Head, Header,
    IntoRecoveredTransaction, Receipt, SealedBlock, EMPTY_OMMER_ROOT, U256,
};
use reth_provider::{BlockProvider, EvmEnvProvider, PostState, StateProviderFactory};
use reth_revm::{
    config::{revm_spec, revm_spec_by_timestamp_after_merge},
    database::{State, SubState},
    env::tx_env_with_recovered,
    executor::{
        commit_state_changes, increment_account_balance, post_block_withdrawals_balance_increments,
    },
    into_reth_log,
};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use revm::primitives::{BlockEnv, CfgEnv, Env, ResultAndState, SpecId};
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
        block_config: BlockConfig,
        chain_spec: Arc<ChainSpec>,
    ) -> Self {
        Self {
            client,
            pool,
            executor,
            payload_task_guard: PayloadTaskGuard::new(config.max_payload_tasks),
            config,
            block_config,
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
        // TODO this needs to access the _pending_ state of the parent block hash
        let parent_block = self
            .client
            .block_by_hash(attributes.parent)?
            .ok_or_else(|| PayloadBuilderError::MissingParentBlock(attributes.parent))?;

        // configure evm env based on parent block
        let initialized_cfg = CfgEnv {
            chain_id: U256::from(self.chain_spec.chain().id()),
            // ensure we're not missing any timestamp based hardforks
            spec_id: revm_spec_by_timestamp_after_merge(&self.chain_spec, attributes.timestamp),
            ..Default::default()
        };

        let initialized_block_env = BlockEnv {
            number: U256::from(parent_block.number + 1),
            coinbase: attributes.suggested_fee_recipient,
            timestamp: U256::from(attributes.timestamp),
            difficulty: U256::ZERO,
            prevrandao: Some(attributes.prev_randao),
            gas_limit: U256::from(parent_block.gas_limit),
            // calculate basefee based on parent block's gas usage
            basefee: U256::from(calculate_next_block_base_fee(
                parent_block.gas_used,
                parent_block.gas_limit,
                parent_block.base_fee_per_gas.unwrap_or_default(),
            )),
        };

        let config = PayloadConfig {
            initialized_block_env,
            initialized_cfg,
            parent_block: Arc::new(parent_block),
            extra_data: self.block_config.extradata.clone(),
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
}

// === impl BasicPayloadJob ===

impl<Client, Pool, Tasks> BasicPayloadJob<Client, Pool, Tasks> {
    /// Checks if the new payload is better than the current best.
    ///
    /// This compares the total fees of the blocks, higher is better.
    fn is_better(&self, new_payload: &BuiltPayload) -> bool {
        if let Some(best_payload) = &self.best_payload {
            new_payload.fees() > best_payload.fees()
        } else {
            true
        }
    }
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
            let payload_config = this.config.clone();
            this.executor.spawn_blocking(Box::pin(async move {
                // acquire the permit for executing the task
                let _permit = guard.0.acquire().await;
                build_payload(client, pool, payload_config, cancel, tx)
            }));
            this.pending_block = Some(PendingPayload { _cancel, payload: rx });
        }

        // poll the pending block
        if let Some(mut fut) = this.pending_block.take() {
            match fut.poll_unpin(cx) {
                Poll::Ready(Ok(payload)) => {
                    this.interval.reset();
                    if this.is_better(&payload) {
                        let payload = Arc::new(payload);
                        this.best_payload = Some(payload.clone());
                        return Poll::Ready(Some(Ok(payload)))
                    }
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
        // TODO if still not set, initialize empty block
        self.best_payload.clone().unwrap()
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

/// Static config for how to build a payload.
#[derive(Clone)]
struct PayloadConfig {
    /// Pre-configured block environment.
    initialized_block_env: BlockEnv,
    /// Configuration for the environment.
    initialized_cfg: CfgEnv,
    /// The parent block.
    parent_block: Arc<Block>,
    /// Block extra data.
    extra_data: Bytes,
    /// Requested attributes for the payload.
    attributes: PayloadBuilderAttributes,
    /// The chain spec.
    chain_spec: Arc<ChainSpec>,
}

/// Builds the payload and sends the result to the given channel.
fn build_payload<Pool, Client>(
    client: Client,
    pool: Pool,
    config: PayloadConfig,
    cancel: Cancelled,
    to_job: oneshot::Sender<Result<BuiltPayload, PayloadBuilderError>>,
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
    ) -> Result<BuiltPayload, PayloadBuilderError>
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

        // TODO this needs to access the _pending_ state of the parent block hash
        let state = client.latest()?;

        let mut db = SubState::new(State::new(state));
        let mut post_state = PostState::default();

        let mut cumulative_gas_used = 0;
        let block_gas_limit: u64 = initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX);

        let mut executed_txs = Vec::new();
        let best_txs = pool.best_transactions();

        let mut total_fees = U256::ZERO;
        let base_fee = initialized_block_env.basefee.to::<u64>();

        for tx in best_txs {
            // ensure we still have capacity for this transaction
            if cumulative_gas_used + tx.gas_limit() > block_gas_limit {
                // TODO: try find transactions that can fit into the block
                break
            }

            // check if the job was cancelled, if so we can exit early
            if cancel.is_cancelled() {
                return Err(PayloadBuilderError::BuildJobCancelled)
            }

            // convert tx to a signed transaction
            let tx = tx.to_recovered_transaction();

            // Configure the environment for the block.
            let env = Env {
                cfg: initialized_cfg.clone(),
                block: initialized_block_env.clone(),
                tx: tx_env_with_recovered(&tx),
            };

            let mut evm = revm::EVM::with_env(env);
            evm.database(&mut db);

            // TODO skip invalid transactions
            let ResultAndState { result, state } =
                evm.transact().map_err(PayloadBuilderError::EvmExecutionError)?;

            // commit changes
            commit_state_changes(&mut db, &mut post_state, state, true);

            // Push transaction changeset and calculate header bloom filter for receipt.
            post_state.add_receipt(Receipt {
                tx_type: tx.tx_type(),
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.logs().into_iter().map(into_reth_log).collect(),
            });

            let gas_used = result.gas_used();

            // update add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(base_fee)
                .expect("fee is always valid; execution succeeded");
            total_fees += U256::from(miner_fee) * U256::from(gas_used);

            // append gas used
            cumulative_gas_used += gas_used;

            // append transaction to the list of executed transactions
            executed_txs.push(tx.into_signed());
        }

        let mut withdrawals_root = None;

        // get balance changes from withdrawals
        if initialized_cfg.spec_id >= SpecId::SHANGHAI {
            let balance_increments = post_block_withdrawals_balance_increments(
                &chain_spec,
                attributes.timestamp,
                &attributes.withdrawals,
            );
            for (address, increment) in balance_increments {
                increment_account_balance(&mut db, &mut post_state, address, increment)?;
            }

            // calculate withdrawals root
            withdrawals_root =
                Some(proofs::calculate_withdrawals_root(attributes.withdrawals.iter()));
        }

        // create the block header
        let transactions_root = proofs::calculate_transaction_root(executed_txs.iter());

        let receipts_root = post_state.receipts_root();
        let logs_bloom = post_state.logs_bloom();

        let header = Header {
            parent_hash: attributes.parent,
            ommers_hash: EMPTY_OMMER_ROOT,
            beneficiary: initialized_block_env.coinbase,
            // TODO compute state root
            state_root: Default::default(),
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
        let block = Block {
            header,
            body: executed_txs,
            ommers: vec![],
            withdrawals: Some(attributes.withdrawals),
        };

        let sealed_block = block.seal_slow();
        Ok(BuiltPayload::new(attributes.id, sealed_block, total_fees))
    }
    let _ = to_job.send(try_build(client, pool, config, cancel));
}
