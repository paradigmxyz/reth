use crate::{
    eth::{
        cache::EthStateCache,
        error::{EthApiError, EthResult},
        revm_utils::{inspect, prepare_call_env, EvmOverrides},
        utils::recover_raw_transaction,
        EthTransactions,
    },
    result::internal_rpc_err,
    TracingCallGuard,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{BlockId, BlockNumberOrTag, Bytes, H256};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_revm::{
    database::{State, SubState},
    env::tx_env_with_recovered,
    tracing::{TracingInspector, TracingInspectorConfig},
};
use reth_rpc_api::TraceApiServer;
use reth_rpc_types::{
    state::StateOverride,
    trace::{filter::TraceFilter, parity::*},
    BlockError, BlockOverrides, CallRequest, Index, TransactionInfo,
};
use reth_tasks::TaskSpawner;
use revm::primitives::Env;
use revm_primitives::{db::DatabaseCommit, ExecutionResult};
use std::{collections::HashSet, future::Future, sync::Arc};
use tokio::sync::{oneshot, AcquireError, OwnedSemaphorePermit};

/// `trace` API implementation.
///
/// This type provides the functionality for handling `trace` related requests.
pub struct TraceApi<Client, Eth> {
    inner: Arc<TraceApiInner<Client, Eth>>,
}

// === impl TraceApi ===

impl<Client, Eth> TraceApi<Client, Eth> {
    /// The client that can interact with the chain.
    pub fn client(&self) -> &Client {
        &self.inner.client
    }

    /// Create a new instance of the [TraceApi]
    pub fn new(
        client: Client,
        eth_api: Eth,
        eth_cache: EthStateCache,
        task_spawner: Box<dyn TaskSpawner>,
        tracing_call_guard: TracingCallGuard,
    ) -> Self {
        let inner = Arc::new(TraceApiInner {
            client,
            eth_api,
            eth_cache,
            task_spawner,
            tracing_call_guard,
        });
        Self { inner }
    }

    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(
        &self,
    ) -> std::result::Result<OwnedSemaphorePermit, AcquireError> {
        self.inner.tracing_call_guard.clone().acquire_owned().await
    }
}

// === impl TraceApi ===

impl<Client, Eth> TraceApi<Client, Eth>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Executes the future on a new blocking task.
    async fn on_blocking_task<C, F, R>(&self, c: C) -> EthResult<R>
    where
        C: FnOnce(Self) -> F,
        F: Future<Output = EthResult<R>> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        let f = c(this);
        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            let res = f.await;
            let _ = tx.send(res);
        }));
        rx.await.map_err(|_| EthApiError::InternalTracingError)?
    }

    /// Executes the given call and returns a number of possible traces for it.
    pub async fn trace_call(
        &self,
        call: CallRequest,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> EthResult<TraceResults> {
        self.on_blocking_task(|this| async move {
            this.try_trace_call(
                call,
                trace_types,
                block_id,
                EvmOverrides::new(state_overrides, block_overrides),
            )
            .await
        })
        .await
    }

    async fn try_trace_call(
        &self,
        call: CallRequest,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
        overrides: EvmOverrides,
    ) -> EthResult<TraceResults> {
        let at = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let config = tracing_config(&trace_types);
        let mut inspector = TracingInspector::new(config);

        let (res, _) =
            self.inner.eth_api.inspect_call_at(call, at, overrides, &mut inspector).await?;

        let trace_res =
            inspector.into_parity_builder().into_trace_results(res.result, &trace_types);
        Ok(trace_res)
    }

    /// Traces a call to `eth_sendRawTransaction` without making the call, returning the traces.
    pub async fn trace_raw_transaction(
        &self,
        tx: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> EthResult<TraceResults> {
        let tx = recover_raw_transaction(tx)?;

        let (cfg, block, at) = self
            .inner
            .eth_api
            .evm_env_at(block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest)))
            .await?;
        let tx = tx_env_with_recovered(&tx);
        let env = Env { cfg, block, tx };

        let config = tracing_config(&trace_types);

        self.on_blocking_task(|this| async move {
            this.inner.eth_api.trace_at(env, config, at, |inspector, res| {
                let trace_res =
                    inspector.into_parity_builder().into_trace_results(res.result, &trace_types);
                Ok(trace_res)
            })
        })
        .await
    }

    /// Performs multiple call traces on top of the same block. i.e. transaction n will be executed
    /// on top of a pending block with all n-1 transactions applied (traced) first.
    ///
    /// Note: Allows tracing dependent transactions, hence all transactions are traced in sequence
    pub async fn trace_call_many(
        &self,
        calls: Vec<(CallRequest, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> EthResult<Vec<TraceResults>> {
        let at = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Pending));
        let (cfg, block_env, at) = self.inner.eth_api.evm_env_at(at).await?;

        self.on_blocking_task(|this| async move {
            // execute all transactions on top of each other and record the traces
            this.inner.eth_api.with_state_at_block(at, move |state| {
                let mut results = Vec::with_capacity(calls.len());
                let mut db = SubState::new(State::new(state));

                for (call, trace_types) in calls {
                    let env = prepare_call_env(
                        cfg.clone(),
                        block_env.clone(),
                        call,
                        &mut db,
                        Default::default(),
                    )?;
                    let config = tracing_config(&trace_types);
                    let mut inspector = TracingInspector::new(config);
                    let (res, _) = inspect(&mut db, env, &mut inspector)?;
                    let trace_res = inspector
                        .into_parity_builder()
                        .into_trace_results(res.result, &trace_types);
                    results.push(trace_res);
                }

                Ok(results)
            })
        })
        .await
    }

    /// Replays a transaction, returning the traces.
    pub async fn replay_transaction(
        &self,
        hash: H256,
        trace_types: HashSet<TraceType>,
    ) -> EthResult<TraceResults> {
        let config = tracing_config(&trace_types);
        self.on_blocking_task(|this| async move {
            this.inner
                .eth_api
                .trace_transaction_in_block(hash, config, |_, inspector, res| {
                    let trace_res = inspector
                        .into_parity_builder()
                        .into_trace_results(res.result, &trace_types);
                    Ok(trace_res)
                })
                .await
                .transpose()
                .ok_or_else(|| EthApiError::TransactionNotFound)?
        })
        .await
    }

    /// Returns transaction trace with the given address.
    pub async fn trace_get(
        &self,
        hash: H256,
        trace_address: Vec<usize>,
    ) -> EthResult<Option<LocalizedTransactionTrace>> {
        match self.trace_transaction(hash).await? {
            None => Ok(None),
            Some(traces) => {
                let trace =
                    traces.into_iter().find(|trace| trace.trace.trace_address == trace_address);
                Ok(trace)
            }
        }
    }

    /// Returns all traces for the given transaction hash
    pub async fn trace_transaction(
        &self,
        hash: H256,
    ) -> EthResult<Option<Vec<LocalizedTransactionTrace>>> {
        self.on_blocking_task(|this| async move {
            this.inner
                .eth_api
                .trace_transaction_in_block(
                    hash,
                    TracingInspectorConfig::default_parity(),
                    |tx_info, inspector, _| {
                        let traces = inspector
                            .into_parity_builder()
                            .into_localized_transaction_traces(tx_info);
                        Ok(traces)
                    },
                )
                .await
        })
        .await
    }

    /// Executes all transactions of a block and returns a list of callback results.
    async fn trace_block_with<F, R>(
        &self,
        block_id: BlockId,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        F: Fn(TransactionInfo, TracingInspector, ExecutionResult) -> EthResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let ((cfg, block_env, _), block) = futures::try_join!(
            self.inner.eth_api.evm_env_at(block_id),
            self.inner.eth_api.block_by_id(block_id),
        )?;

        let block = match block {
            Some(block) => block,
            None => return Ok(None),
        };

        // we need to get the state of the parent block because we're replaying this block on top of
        // its parent block's state
        let state_at = block.parent_hash;

        let block_hash = block.hash;
        let transactions = block.body;

        self.on_blocking_task(|this| async move {
            // replay all transactions of the block
            this.inner
                .eth_api
                .with_state_at_block(state_at.into(), move |state| {
                    let mut results = Vec::with_capacity(transactions.len());
                    let mut db = SubState::new(State::new(state));

                    let mut transactions = transactions.into_iter().enumerate().peekable();

                    while let Some((idx, tx)) = transactions.next() {
                        let tx = tx.into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
                        let tx_info = TransactionInfo {
                            hash: Some(tx.hash()),
                            index: Some(idx as u64),
                            block_hash: Some(block_hash),
                            block_number: Some(block_env.number.try_into().unwrap_or(u64::MAX)),
                            base_fee: Some(block_env.basefee.try_into().unwrap_or(u64::MAX)),
                        };

                        let tx = tx_env_with_recovered(&tx);
                        let env = Env { cfg: cfg.clone(), block: block_env.clone(), tx };

                        let mut inspector = TracingInspector::new(config);
                        let (res, _) = inspect(&mut db, env, &mut inspector)?;
                        results.push(f(tx_info, inspector, res.result)?);

                        // need to apply the state changes of this transaction before executing the
                        // next transaction
                        if transactions.peek().is_some() {
                            // need to apply the state changes of this transaction before executing
                            // the next transaction
                            db.commit(res.state)
                        }
                    }

                    Ok(results)
                })
                .map(Some)
        })
        .await
    }

    /// Returns traces created at given block.
    pub async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> EthResult<Option<Vec<LocalizedTransactionTrace>>> {
        let traces = self
            .trace_block_with(
                block_id,
                TracingInspectorConfig::default_parity(),
                |tx_info, inspector, _| {
                    let traces =
                        inspector.into_parity_builder().into_localized_transaction_traces(tx_info);
                    Ok(traces)
                },
            )
            .await?
            .map(|traces| traces.into_iter().flatten().collect());
        Ok(traces)
    }

    /// Replays all transactions in a block
    pub async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> EthResult<Option<Vec<TraceResultsWithTransactionHash>>> {
        self.trace_block_with(
            block_id,
            tracing_config(&trace_types),
            move |tx_info, inspector, res| {
                let full_trace =
                    inspector.into_parity_builder().into_trace_results(res, &trace_types);
                let trace = TraceResultsWithTransactionHash {
                    transaction_hash: tx_info.hash.expect("tx hash is set"),
                    full_trace,
                };
                Ok(trace)
            },
        )
        .await
    }
}

#[async_trait]
impl<Client, Eth> TraceApiServer for TraceApi<Client, Eth>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Executes the given call and returns a number of possible traces for it.
    ///
    /// Handler for `trace_call`
    async fn trace_call(
        &self,
        call: CallRequest,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> Result<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_call(
            self,
            call,
            trace_types,
            block_id,
            state_overrides,
            block_overrides,
        )
        .await?)
    }

    /// Handler for `trace_callMany`
    async fn trace_call_many(
        &self,
        calls: Vec<(CallRequest, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> Result<Vec<TraceResults>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_call_many(self, calls, block_id).await?)
    }

    /// Handler for `trace_rawTransaction`
    async fn trace_raw_transaction(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> Result<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_raw_transaction(self, data, trace_types, block_id).await?)
    }

    /// Handler for `trace_replayBlockTransactions`
    async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> Result<Option<Vec<TraceResultsWithTransactionHash>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::replay_block_transactions(self, block_id, trace_types).await?)
    }

    /// Handler for `trace_replayTransaction`
    async fn replay_transaction(
        &self,
        transaction: H256,
        trace_types: HashSet<TraceType>,
    ) -> Result<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::replay_transaction(self, transaction, trace_types).await?)
    }

    /// Handler for `trace_block`
    async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_block(self, block_id).await?)
    }

    /// Handler for `trace_filter`
    async fn trace_filter(&self, _filter: TraceFilter) -> Result<Vec<LocalizedTransactionTrace>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Returns transaction trace at given index.
    /// Handler for `trace_get`
    async fn trace_get(
        &self,
        hash: H256,
        indices: Vec<Index>,
    ) -> Result<Option<LocalizedTransactionTrace>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_get(self, hash, indices.into_iter().map(Into::into).collect()).await?)
    }

    /// Handler for `trace_transaction`
    async fn trace_transaction(
        &self,
        hash: H256,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_transaction(self, hash).await?)
    }
}

impl<Client, Eth> std::fmt::Debug for TraceApi<Client, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceApi").finish_non_exhaustive()
    }
}
impl<Client, Eth> Clone for TraceApi<Client, Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct TraceApiInner<Client, Eth> {
    /// The client that can interact with the chain.
    client: Client,
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    /// The async cache frontend for eth-related data
    #[allow(unused)] // we need this for trace_filter eventually
    eth_cache: EthStateCache,
    /// The type that can spawn tasks which would otherwise be blocking.
    task_spawner: Box<dyn TaskSpawner>,
    // restrict the number of concurrent calls to `trace_*`
    tracing_call_guard: TracingCallGuard,
}

/// Returns the [TracingInspectorConfig] depending on the enabled [TraceType]s
fn tracing_config(trace_types: &HashSet<TraceType>) -> TracingInspectorConfig {
    TracingInspectorConfig::default_parity()
        .set_state_diffs(trace_types.contains(&TraceType::StateDiff))
        .set_steps(trace_types.contains(&TraceType::VmTrace))
}
