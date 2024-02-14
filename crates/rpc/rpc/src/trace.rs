use crate::{
    eth::{
        error::{EthApiError, EthResult},
        revm_utils::{inspect, inspect_and_return_db, prepare_call_env, EvmOverrides},
        utils::recover_raw_transaction,
        EthTransactions,
    },
    BlockingTaskGuard,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_consensus_common::calc::{base_block_reward, block_reward};
use reth_primitives::{
    revm::env::tx_env_with_recovered, BlockId, BlockNumberOrTag, Bytes, SealedHeader, B256, U256,
};
use reth_provider::{BlockReader, ChainSpecProvider, EvmEnvProvider, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    tracing::{parity::populate_state_diff, TracingInspector, TracingInspectorConfig},
};
use reth_rpc_api::TraceApiServer;
use reth_rpc_types::{
    state::StateOverride,
    trace::{filter::TraceFilter, parity::*, tracerequest::TraceCallRequest},
    BlockError, BlockOverrides, Index, TransactionRequest,
};
use revm::{
    db::{CacheDB, DatabaseCommit},
    primitives::EnvWithHandlerCfg,
};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{AcquireError, OwnedSemaphorePermit};

/// `trace` API implementation.
///
/// This type provides the functionality for handling `trace` related requests.
pub struct TraceApi<Provider, Eth> {
    inner: Arc<TraceApiInner<Provider, Eth>>,
}

// === impl TraceApi ===

impl<Provider, Eth> TraceApi<Provider, Eth> {
    /// The provider that can interact with the chain.
    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// Create a new instance of the [TraceApi]
    pub fn new(provider: Provider, eth_api: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        let inner = Arc::new(TraceApiInner { provider, eth_api, blocking_task_guard });
        Self { inner }
    }

    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(
        &self,
    ) -> std::result::Result<OwnedSemaphorePermit, AcquireError> {
        self.inner.blocking_task_guard.clone().acquire_owned().await
    }
}

// === impl TraceApi ===

impl<Provider, Eth> TraceApi<Provider, Eth>
where
    Provider: BlockReader + StateProviderFactory + EvmEnvProvider + ChainSpecProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Executes the given call and returns a number of possible traces for it.
    pub async fn trace_call(&self, trace_request: TraceCallRequest) -> EthResult<TraceResults> {
        let at = trace_request.block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let config = TracingInspectorConfig::from_parity_config(&trace_request.trace_types);
        let overrides =
            EvmOverrides::new(trace_request.state_overrides, trace_request.block_overrides);
        let mut inspector = TracingInspector::new(config);
        self.inner
            .eth_api
            .spawn_with_call_at(trace_request.call, at, overrides, move |db, env| {
                let (res, _, db) = inspect_and_return_db(db, env, &mut inspector)?;
                let trace_res = inspector.into_parity_builder().into_trace_results_with_state(
                    &res,
                    &trace_request.trace_types,
                    &db,
                )?;
                Ok(trace_res)
            })
            .await
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
        let tx = tx_env_with_recovered(&tx.into_ecrecovered_transaction());
        let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block, tx);

        let config = TracingInspectorConfig::from_parity_config(&trace_types);

        self.inner
            .eth_api
            .spawn_trace_at_with_state(env, config, at, move |inspector, res, db| {
                Ok(inspector.into_parity_builder().into_trace_results_with_state(
                    &res,
                    &trace_types,
                    &db,
                )?)
            })
            .await
    }

    /// Performs multiple call traces on top of the same block. i.e. transaction n will be executed
    /// on top of a pending block with all n-1 transactions applied (traced) first.
    ///
    /// Note: Allows tracing dependent transactions, hence all transactions are traced in sequence
    pub async fn trace_call_many(
        &self,
        calls: Vec<(TransactionRequest, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> EthResult<Vec<TraceResults>> {
        let at = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Pending));
        let (cfg, block_env, at) = self.inner.eth_api.evm_env_at(at).await?;

        let gas_limit = self.inner.eth_api.call_gas_limit();
        // execute all transactions on top of each other and record the traces
        self.inner
            .eth_api
            .spawn_with_state_at_block(at, move |state| {
                let mut results = Vec::with_capacity(calls.len());
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                let mut calls = calls.into_iter().peekable();

                while let Some((call, trace_types)) = calls.next() {
                    let env = prepare_call_env(
                        cfg.clone(),
                        block_env.clone(),
                        call,
                        gas_limit,
                        &mut db,
                        Default::default(),
                    )?;
                    let config = TracingInspectorConfig::from_parity_config(&trace_types);
                    let mut inspector = TracingInspector::new(config);
                    let (res, _) = inspect(&mut db, env, &mut inspector)?;

                    let trace_res = inspector.into_parity_builder().into_trace_results_with_state(
                        &res,
                        &trace_types,
                        &db,
                    )?;

                    results.push(trace_res);

                    // need to apply the state changes of this call before executing the
                    // next call
                    if calls.peek().is_some() {
                        // need to apply the state changes of this call before executing
                        // the next call
                        db.commit(res.state)
                    }
                }

                Ok(results)
            })
            .await
    }

    /// Replays a transaction, returning the traces.
    pub async fn replay_transaction(
        &self,
        hash: B256,
        trace_types: HashSet<TraceType>,
    ) -> EthResult<TraceResults> {
        let config = TracingInspectorConfig::from_parity_config(&trace_types);
        self.inner
            .eth_api
            .spawn_trace_transaction_in_block(hash, config, move |_, inspector, res, db| {
                let trace_res = inspector.into_parity_builder().into_trace_results_with_state(
                    &res,
                    &trace_types,
                    &db,
                )?;
                Ok(trace_res)
            })
            .await
            .transpose()
            .ok_or_else(|| EthApiError::TransactionNotFound)?
    }

    /// Returns transaction trace objects at the given index
    ///
    /// Note: For compatibility reasons this only supports 1 single index, since this method is
    /// supposed to return a single trace. See also: <https://github.com/ledgerwatch/erigon/blob/862faf054b8a0fa15962a9c73839b619886101eb/turbo/jsonrpc/trace_filtering.go#L114-L133>
    ///
    /// This returns `None` if `indices` is empty
    pub async fn trace_get(
        &self,
        hash: B256,
        indices: Vec<usize>,
    ) -> EthResult<Option<LocalizedTransactionTrace>> {
        if indices.len() != 1 {
            // The OG impl failed if it gets more than a single index
            return Ok(None)
        }
        self.trace_get_index(hash, indices[0]).await
    }

    /// Returns transaction trace object at the given index.
    ///
    /// Returns `None` if the trace object at that index does not exist
    pub async fn trace_get_index(
        &self,
        hash: B256,
        index: usize,
    ) -> EthResult<Option<LocalizedTransactionTrace>> {
        Ok(self.trace_transaction(hash).await?.and_then(|traces| traces.into_iter().nth(index)))
    }

    /// Returns all transaction traces that match the given filter.
    ///
    /// This is similar to [Self::trace_block] but only returns traces for transactions that match
    /// the filter.
    pub async fn trace_filter(
        &self,
        filter: TraceFilter,
    ) -> EthResult<Vec<LocalizedTransactionTrace>> {
        let matcher = filter.matcher();
        let TraceFilter { from_block, to_block, after: _after, count: _count, .. } = filter;
        let start = from_block.unwrap_or(0);
        let end = if let Some(to_block) = to_block {
            to_block
        } else {
            self.provider().best_block_number()?
        };

        // ensure that the range is not too large, since we need to fetch all blocks in the range
        let distance = end.saturating_sub(start);
        if distance > 100 {
            return Err(EthApiError::InvalidParams(
                "Block range too large; currently limited to 100 blocks".to_string(),
            ))
        }

        // fetch all blocks in that range
        let blocks = self.provider().block_range(start..=end)?;

        // find relevant blocks to trace
        let mut target_blocks = Vec::new();
        for block in blocks {
            let mut transaction_indices = HashSet::new();
            let mut highest_matching_index = 0;
            for (tx_idx, tx) in block.body.iter().enumerate() {
                let from = tx.recover_signer().ok_or(BlockError::InvalidSignature)?;
                let to = tx.to();
                if matcher.matches(from, to) {
                    let idx = tx_idx as u64;
                    transaction_indices.insert(idx);
                    highest_matching_index = idx;
                }
            }
            if !transaction_indices.is_empty() {
                target_blocks.push((block.number, transaction_indices, highest_matching_index));
            }
        }

        // trace all relevant blocks
        let mut block_traces = Vec::with_capacity(target_blocks.len());
        for (num, indices, highest_idx) in target_blocks {
            let traces = self.inner.eth_api.trace_block_until(
                num.into(),
                Some(highest_idx),
                TracingInspectorConfig::default_parity(),
                move |tx_info, inspector, res, _, _| {
                    if let Some(idx) = tx_info.index {
                        if !indices.contains(&idx) {
                            // only record traces for relevant transactions
                            return Ok(None)
                        }
                    }
                    let traces = inspector
                        .with_transaction_gas_used(res.gas_used())
                        .into_parity_builder()
                        .into_localized_transaction_traces(tx_info);
                    Ok(Some(traces))
                },
            );
            block_traces.push(traces);
        }

        let block_traces = futures::future::try_join_all(block_traces).await?;
        let all_traces = block_traces
            .into_iter()
            .flatten()
            .flat_map(|traces| traces.into_iter().flatten().flat_map(|traces| traces.into_iter()))
            .collect();

        Ok(all_traces)
    }

    /// Returns all traces for the given transaction hash
    pub async fn trace_transaction(
        &self,
        hash: B256,
    ) -> EthResult<Option<Vec<LocalizedTransactionTrace>>> {
        self.inner
            .eth_api
            .spawn_trace_transaction_in_block(
                hash,
                TracingInspectorConfig::default_parity(),
                move |tx_info, inspector, res, _| {
                    let traces = inspector
                        .with_transaction_gas_used(res.result.gas_used())
                        .into_parity_builder()
                        .into_localized_transaction_traces(tx_info);
                    Ok(traces)
                },
            )
            .await
    }

    /// Returns traces created at given block.
    pub async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> EthResult<Option<Vec<LocalizedTransactionTrace>>> {
        let traces = self.inner.eth_api.trace_block_with(
            block_id,
            TracingInspectorConfig::default_parity(),
            |tx_info, inspector, res, _, _| {
                let traces = inspector
                    .with_transaction_gas_used(res.gas_used())
                    .into_parity_builder()
                    .into_localized_transaction_traces(tx_info);
                Ok(traces)
            },
        );

        let block = self.inner.eth_api.block_by_id(block_id);
        let (maybe_traces, maybe_block) = futures::try_join!(traces, block)?;

        let mut maybe_traces =
            maybe_traces.map(|traces| traces.into_iter().flatten().collect::<Vec<_>>());

        if let (Some(block), Some(traces)) = (maybe_block, maybe_traces.as_mut()) {
            if let Some(header_td) = self.provider().header_td(&block.header.hash())? {
                if let Some(base_block_reward) = base_block_reward(
                    self.provider().chain_spec().as_ref(),
                    block.header.number,
                    block.header.difficulty,
                    header_td,
                ) {
                    traces.push(reward_trace(
                        &block.header,
                        RewardAction {
                            author: block.header.beneficiary,
                            reward_type: RewardType::Block,
                            value: U256::from(base_block_reward),
                        },
                    ));

                    if !block.ommers.is_empty() {
                        traces.push(reward_trace(
                            &block.header,
                            RewardAction {
                                author: block.header.beneficiary,
                                reward_type: RewardType::Uncle,
                                value: U256::from(
                                    block_reward(base_block_reward, block.ommers.len()) -
                                        base_block_reward,
                                ),
                            },
                        ));
                    }
                }
            }
        }

        Ok(maybe_traces)
    }

    /// Replays all transactions in a block
    pub async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> EthResult<Option<Vec<TraceResultsWithTransactionHash>>> {
        self.inner
            .eth_api
            .trace_block_with(
                block_id,
                TracingInspectorConfig::from_parity_config(&trace_types),
                move |tx_info, inspector, res, state, db| {
                    let mut full_trace =
                        inspector.into_parity_builder().into_trace_results(&res, &trace_types);

                    // If statediffs were requested, populate them with the account balance and
                    // nonce from pre-state
                    if let Some(ref mut state_diff) = full_trace.state_diff {
                        populate_state_diff(state_diff, db, state.iter())?;
                    }

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
impl<Provider, Eth> TraceApiServer for TraceApi<Provider, Eth>
where
    Provider: BlockReader + StateProviderFactory + EvmEnvProvider + ChainSpecProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Executes the given call and returns a number of possible traces for it.
    ///
    /// Handler for `trace_call`
    async fn trace_call(
        &self,
        call: TransactionRequest,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> Result<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        let request =
            TraceCallRequest { call, trace_types, block_id, state_overrides, block_overrides };
        Ok(TraceApi::trace_call(self, request).await?)
    }

    /// Handler for `trace_callMany`
    async fn trace_call_many(
        &self,
        calls: Vec<(TransactionRequest, HashSet<TraceType>)>,
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
        transaction: B256,
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
    ///
    /// This is similar to `eth_getLogs` but for traces.
    ///
    /// # Limitations
    /// This currently requires block filter fields, since reth does not have address indices yet.
    async fn trace_filter(&self, filter: TraceFilter) -> Result<Vec<LocalizedTransactionTrace>> {
        Ok(TraceApi::trace_filter(self, filter).await?)
    }

    /// Returns transaction trace at given index.
    /// Handler for `trace_get`
    async fn trace_get(
        &self,
        hash: B256,
        indices: Vec<Index>,
    ) -> Result<Option<LocalizedTransactionTrace>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_get(self, hash, indices.into_iter().map(Into::into).collect()).await?)
    }

    /// Handler for `trace_transaction`
    async fn trace_transaction(
        &self,
        hash: B256,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(TraceApi::trace_transaction(self, hash).await?)
    }
}

impl<Provider, Eth> std::fmt::Debug for TraceApi<Provider, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceApi").finish_non_exhaustive()
    }
}
impl<Provider, Eth> Clone for TraceApi<Provider, Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct TraceApiInner<Provider, Eth> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    // restrict the number of concurrent calls to `trace_*`
    blocking_task_guard: BlockingTaskGuard,
}

/// Helper to construct a [`LocalizedTransactionTrace`] that describes a reward to the block
/// beneficiary.
fn reward_trace(header: &SealedHeader, reward: RewardAction) -> LocalizedTransactionTrace {
    LocalizedTransactionTrace {
        block_hash: Some(header.hash()),
        block_number: Some(header.number),
        transaction_hash: None,
        transaction_position: None,
        trace: TransactionTrace {
            trace_address: vec![],
            subtraces: 0,
            action: Action::Reward(reward),
            error: None,
            result: None,
        },
    }
}
