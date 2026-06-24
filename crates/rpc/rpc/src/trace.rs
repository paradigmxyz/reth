use alloy_consensus::{constants::ETH_TO_WEI, BlockHeader};
use alloy_eips::BlockId;
#[cfg(any())]
use alloy_primitives::{map::HashMap, Address, BlockHash};
use alloy_primitives::{map::HashSet, BlockHash, Bytes, B256, U256};
use alloy_rpc_types_eth::{state::StateOverride, BlockOverrides, Index};
use alloy_rpc_types_trace::{
    filter::TraceFilter,
    opcode::{BlockOpcodeGas, TransactionOpcodeGas},
    parity::*,
};
use async_trait::async_trait;
use evm2::{ethereum::RecoveredTxEnvelope, evm::Db};
use evm2_inspectors::{opcode::OpcodeGasInspector, tracing::TracingInspectorConfig};
use futures::StreamExt;
use jsonrpsee::core::RpcResult;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_evm::TxEnvFor;
use reth_primitives_traits::BlockBody;
#[cfg(any())]
use reth_primitives_traits::BlockHeader;
use reth_rpc_api::TraceApiServer;
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_api::{
    helpers::{TraceExt, TracingCtx},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, EthConfig};
use reth_storage_api::{BlockNumReader, BlockReader, ProviderTx};
use reth_tasks::pool::BlockingTaskGuard;
#[cfg(any())]
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Maximum number of `trace_filter` blocks replayed concurrently.
const TRACE_FILTER_BLOCK_BUFFER_SIZE: usize = 4;
/// Number of blocks fetched per provider range read in `trace_filter`.
const TRACE_FILTER_FETCH_CHUNK_SIZE: usize = 16;

/// `trace` API implementation.
///
/// This type provides the functionality for handling `trace` related requests.
pub struct TraceApi<Eth> {
    inner: Arc<TraceApiInner<Eth>>,
}

// === impl TraceApi ===

impl<Eth> TraceApi<Eth> {
    /// Create a new instance of the [`TraceApi`]
    pub fn new(
        eth_api: Eth,
        blocking_task_guard: BlockingTaskGuard,
        eth_config: EthConfig,
    ) -> Self {
        let inner = Arc::new(TraceApiInner { eth_api, blocking_task_guard, eth_config });
        Self { inner }
    }

    /// Access the underlying `Eth` API.
    pub fn eth_api(&self) -> &Eth {
        &self.inner.eth_api
    }
}

fn unsupported_trace<T>() -> RpcResult<T> {
    Err(EthApiError::Unsupported("trace API is unsupported by the active EVM execution path")
        .into())
}

// === impl TraceApi === //

#[cfg(any())]
impl<Eth> TraceApi<Eth>
where
    // tracing methods do _not_ read from mempool, hence no `LoadBlock` trait
    // bound
    Eth: Trace + Call + LoadPendingBlock + LoadTransaction + 'static,
{
    /// Executes the given call and returns a number of possible traces for it.
    pub async fn trace_call(
        &self,
        trace_request: TraceCallRequest<RpcTxReq<Eth::NetworkTypes>>,
    ) -> Result<TraceResults, Eth::Error> {
        let at = trace_request.block_id.unwrap_or_default();
        let config = TracingInspectorConfig::from_parity_config(&trace_request.trace_types);
        let overrides =
            EvmOverrides::new(trace_request.state_overrides, trace_request.block_overrides);
        let mut inspector = TracingInspector::new(config);
        let this = self.clone();
        self.eth_api()
            .spawn_with_call_at(trace_request.call, at, overrides, move |db, evm_env, tx_env| {
                let res = this.eth_api().inspect(&mut *db, evm_env, tx_env, &mut inspector)?;
                let trace_res = inspector
                    .into_parity_builder()
                    .into_trace_results_with_state(&res, &trace_request.trace_types, &db)
                    .map_err(Eth::Error::from_eth_err)?;
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
    ) -> Result<TraceResults, Eth::Error> {
        let tx = recover_raw_transaction::<PoolPooledTx<Eth::Pool>>(&tx)?
            .map(<Eth::Pool as TransactionPool>::Transaction::pooled_into_consensus);

        let (evm_env, at) = self.eth_api().evm_env_at(block_id.unwrap_or_default()).await?;
        let tx_env = self.eth_api().evm_config().tx_env(tx);

        let config = TracingInspectorConfig::from_parity_config(&trace_types);

        self.eth_api()
            .spawn_trace_at_with_state(evm_env, tx_env, config, at, move |inspector, res, db| {
                inspector
                    .into_parity_builder()
                    .into_trace_results_with_state(&res, &trace_types, &db)
                    .map_err(Eth::Error::from_eth_err)
            })
            .await
    }

    /// Performs multiple call traces on top of the same block. i.e. transaction n will be executed
    /// on top of a pending block with all n-1 transactions applied (traced) first.
    ///
    /// Note: Allows tracing dependent transactions, hence all transactions are traced in sequence
    pub async fn trace_call_many(
        &self,
        calls: Vec<(RpcTxReq<Eth::NetworkTypes>, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> Result<Vec<TraceResults>, Eth::Error> {
        let at = block_id.unwrap_or(BlockId::pending());
        let (evm_env, at) = self.eth_api().evm_env_at(at).await?;

        // execute all transactions on top of each other and record the traces
        self.eth_api()
            .spawn_with_state_at_block(at, move |eth_api, mut db| {
                let mut results = Vec::with_capacity(calls.len());
                let mut calls = calls.into_iter().peekable();

                while let Some((call, trace_types)) = calls.next() {
                    let (evm_env, tx_env) = eth_api.prepare_call_env(
                        evm_env.clone(),
                        call,
                        &mut db,
                        Default::default(),
                    )?;
                    let config = TracingInspectorConfig::from_parity_config(&trace_types);
                    let mut inspector = TracingInspector::new(config);
                    let res = eth_api.inspect(&mut db, evm_env, tx_env, &mut inspector)?;

                    let trace_res = inspector
                        .into_parity_builder()
                        .into_trace_results_with_state(&res, &trace_types, &db)
                        .map_err(Eth::Error::from_eth_err)?;

                    results.push(trace_res);

                    // need to apply the state changes of this call before executing the
                    // next call
                    if calls.peek().is_some() {
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
    ) -> Result<TraceResults, Eth::Error> {
        let config = TracingInspectorConfig::from_parity_config(&trace_types);
        self.eth_api()
            .spawn_trace_transaction_in_block(hash, config, move |_, inspector, res, db| {
                let trace_res = inspector
                    .into_parity_builder()
                    .into_trace_results_with_state(&res, &trace_types, &db)
                    .map_err(Eth::Error::from_eth_err)?;
                Ok(trace_res)
            })
            .await
            .transpose()
            .ok_or(EthApiError::TransactionNotFound)?
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
    ) -> Result<Option<LocalizedTransactionTrace>, Eth::Error> {
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
    ) -> Result<Option<LocalizedTransactionTrace>, Eth::Error> {
        Ok(self.trace_transaction(hash).await?.and_then(|traces| traces.into_iter().nth(index)))
    }

    /// Returns all traces for the given transaction hash
    pub async fn trace_transaction(
        &self,
        hash: B256,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>, Eth::Error> {
        self.eth_api()
            .spawn_trace_transaction_in_block(
                hash,
                TracingInspectorConfig::default_parity(),
                move |tx_info, inspector, _, _| {
                    let traces =
                        inspector.into_parity_builder().into_localized_transaction_traces(tx_info);
                    Ok(traces)
                },
            )
            .await
    }

    /// Returns all opcodes with their count and combined gas usage for the given transaction in no
    /// particular order.
    pub async fn trace_transaction_opcode_gas(
        &self,
        tx_hash: B256,
    ) -> Result<Option<TransactionOpcodeGas>, Eth::Error> {
        self.eth_api()
            .spawn_trace_transaction_in_block_with_inspector(
                tx_hash,
                OpcodeGasInspector::default(),
                move |_tx_info, inspector, _res, _| {
                    let trace = TransactionOpcodeGas {
                        transaction_hash: tx_hash,
                        opcode_gas: inspector.opcode_gas_iter().collect(),
                    };
                    Ok(trace)
                },
            )
            .await
    }

    /// Calculates the base block reward for the given block:
    ///
    /// - if Paris hardfork is activated, no block rewards are given
    /// - if Paris hardfork is not activated, calculate block rewards with block number only
    fn calculate_base_block_reward<H: BlockHeader>(
        &self,
        header: &H,
    ) -> Result<Option<u128>, Eth::Error> {
        let chain_spec = self.provider().chain_spec();

        if chain_spec.is_paris_active_at_block(header.number()) {
            return Ok(None)
        }

        Ok(Some(base_block_reward_pre_merge(&chain_spec, header.number())))
    }

    /// Extracts the reward traces for the given block:
    ///  - block reward
    ///  - uncle rewards
    fn extract_reward_traces<H: BlockHeader>(
        &self,
        header: &H,
        block_hash: BlockHash,
        ommers: Option<&[H]>,
        base_block_reward: u128,
    ) -> Vec<LocalizedTransactionTrace> {
        let ommers_cnt = ommers.map(|o| o.len()).unwrap_or_default();
        let mut traces = Vec::with_capacity(ommers_cnt + 1);

        let block_reward = block_reward(base_block_reward, ommers_cnt);
        traces.push(reward_trace(
            block_hash,
            header,
            RewardAction {
                author: header.beneficiary(),
                reward_type: RewardType::Block,
                value: U256::from(block_reward),
            },
        ));

        let Some(ommers) = ommers else { return traces };

        for uncle in ommers {
            let uncle_reward = ommer_reward(base_block_reward, header.number(), uncle.number());
            traces.push(reward_trace(
                block_hash,
                header,
                RewardAction {
                    author: uncle.beneficiary(),
                    reward_type: RewardType::Uncle,
                    value: U256::from(uncle_reward),
                },
            ));
        }
        traces
    }
}

#[cfg(any())]
impl<Eth> TraceApi<Eth>
where
    // tracing methods read from mempool, hence `LoadBlock` trait bound via
    // `TraceExt`
    Eth: TraceExt + 'static,
{
    /// Returns all transaction traces that match the given filter.
    ///
    /// This is similar to [`Self::trace_block`] but only returns traces for transactions that match
    /// the filter.
    pub async fn trace_filter(
        &self,
        filter: TraceFilter,
    ) -> Result<Vec<LocalizedTransactionTrace>, Eth::Error> {
        // We'll reuse the matcher across multiple blocks that are traced in parallel
        let matcher = Arc::new(filter.matcher());
        let TraceFilter { from_block, to_block, mut after, count, .. } = filter;
        let start = from_block.unwrap_or(0);

        let latest_block = self.provider().best_block_number().map_err(Eth::Error::from_eth_err)?;
        if start > latest_block {
            // can't trace that range
            return Err(EthApiError::HeaderNotFound(start.into()).into());
        }
        let end = to_block.unwrap_or(latest_block);
        if end > latest_block {
            return Err(EthApiError::HeaderNotFound(end.into()).into());
        }

        // Check if the requested range overlaps with pruned history (EIP-4444)
        let earliest_block =
            self.provider().earliest_block_number().map_err(Eth::Error::from_eth_err)?;
        if start < earliest_block {
            return Err(EthApiError::PrunedHistoryUnavailable.into());
        }

        if start > end {
            return Err(EthApiError::InvalidParams(
                "invalid parameters: fromBlock cannot be greater than toBlock".to_string(),
            )
            .into())
        }

        // ensure that the range is not too large, since every block in the range may be replayed
        let distance = end.saturating_sub(start);
        if distance > self.inner.eth_config.max_trace_filter_blocks {
            return Err(EthApiError::InvalidParams(format!(
                "Block range too large; currently limited to {} blocks",
                self.inner.eth_config.max_trace_filter_blocks
            ))
            .into())
        }

        let mut all_traces = Vec::new();
        let block_buffer_size =
            self.inner.eth_config.max_tracing_requests.clamp(1, TRACE_FILTER_BLOCK_BUFFER_SIZE);
        let mut include_reward_traces = true;

        for chunk_start in (start..=end).step_by(TRACE_FILTER_FETCH_CHUNK_SIZE) {
            let chunk_end = (chunk_start + TRACE_FILTER_FETCH_CHUNK_SIZE as u64 - 1).min(end);

            let blocks = self
                .eth_api()
                .spawn_blocking_io(move |this| {
                    let blocks = this
                        .provider()
                        .recovered_block_range(chunk_start..=chunk_end)
                        .map_err(Eth::Error::from_eth_err)?;

                    Ok(blocks.into_iter().map(Arc::new).collect::<Vec<_>>())
                })
                .await?;

            let mut block_replays = futures::stream::iter(blocks)
                .map(|block| {
                    let this = self.clone();
                    let matcher = matcher.clone();

                    let block_hash = block.hash();

                    async move {
                        let permit = this.acquire_trace_permit().await;
                        let traces = this
                            .eth_api()
                            .trace_block_until(
                                block_hash.into(),
                                Some(block.clone()),
                                None,
                                TracingInspectorConfig::default_parity(),
                                move |tx_info, mut ctx| {
                                    // Keep the block replay permit inside the spawned replay task.
                                    let _block_replay_permit = &permit;
                                    let mut traces = ctx
                                        .take_inspector()
                                        .into_parity_builder()
                                        .into_localized_transaction_traces(tx_info);
                                    traces.retain(|trace| matcher.matches(&trace.trace));
                                    Ok(Some(traces))
                                },
                            )
                            .await?;

                        Ok::<_, Eth::Error>((block, traces))
                    }
                })
                .buffered(block_buffer_size);

            while let Some(block_replay) = block_replays.next().await {
                let (block, traces) = block_replay?;
                let reward_traces = if include_reward_traces {
                    if let Some(base_block_reward) =
                        self.calculate_base_block_reward(block.header())?
                    {
                        self.extract_reward_traces(
                            block.header(),
                            block.hash(),
                            block.body().ommers(),
                            base_block_reward,
                        )
                        .into_iter()
                        .filter(|trace| matcher.matches(&trace.trace))
                        .collect::<Vec<_>>()
                    } else {
                        // Blocks are processed in ascending order, so once a historical range
                        // reaches post-Paris blocks, later blocks in the range have no rewards.
                        include_reward_traces = false;
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };

                if let Some(traces) = traces {
                    all_traces.extend(traces.into_iter().flatten().flatten());
                }
                all_traces.extend(reward_traces);

                if let Some(traces) =
                    apply_trace_filter_pagination(&mut all_traces, &mut after, count)
                {
                    return Ok(traces)
                }
            }
        }

        // If `after` is greater than or equal to the number of matched traces, it returns an
        // empty array.
        if let Some(cutoff) = after.map(|a| a as usize) &&
            cutoff >= all_traces.len()
        {
            return Ok(vec![])
        }

        Ok(all_traces)
    }

    /// Returns traces created at given block.
    pub async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>, Eth::Error> {
        let Some(block) = self.eth_api().recovered_block(block_id).await? else {
            return Err(EthApiError::HeaderNotFound(block_id).into());
        };

        let mut traces = self
            .eth_api()
            .trace_block_with(
                block_id,
                Some(block.clone()),
                TracingInspectorConfig::default_parity(),
                |tx_info, mut ctx| {
                    let traces = ctx
                        .take_inspector()
                        .into_parity_builder()
                        .into_localized_transaction_traces(tx_info);
                    Ok(traces)
                },
            )
            .await?
            .map(|traces| traces.into_iter().flatten().collect::<Vec<_>>());

        if let Some(traces) = traces.as_mut() &&
            let Some(base_block_reward) = self.calculate_base_block_reward(block.header())?
        {
            traces.extend(self.extract_reward_traces(
                block.header(),
                block.hash(),
                block.body().ommers(),
                base_block_reward,
            ));
        }

        Ok(traces)
    }

    /// Replays all transactions in a block
    pub async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> Result<Option<Vec<TraceResultsWithTransactionHash>>, Eth::Error> {
        self.eth_api()
            .trace_block_with(
                block_id,
                None,
                TracingInspectorConfig::from_parity_config(&trace_types),
                move |tx_info, mut ctx| {
                    let mut full_trace = ctx
                        .take_inspector()
                        .into_parity_builder()
                        .into_trace_results(&ctx.result, &trace_types);

                    // If statediffs were requested, populate them with the account balance and
                    // nonce from pre-state
                    if let Some(ref mut state_diff) = full_trace.state_diff {
                        populate_state_diff(state_diff, &ctx.db, ctx.state.iter())
                            .map_err(Eth::Error::from_eth_err)?;
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

    /// Returns the opcodes of all transactions in the given block.
    ///
    /// This is the same as [`Self::trace_transaction_opcode_gas`] but for all transactions in a
    /// block.
    pub async fn trace_block_opcode_gas(
        &self,
        block_id: BlockId,
    ) -> Result<Option<BlockOpcodeGas>, Eth::Error> {
        let Some(block) = self.eth_api().recovered_block(block_id).await? else {
            return Err(EthApiError::HeaderNotFound(block_id).into());
        };

        let Some(transactions) = self
            .eth_api()
            .trace_block_inspector(
                block_id,
                Some(block.clone()),
                OpcodeGasInspector::default,
                move |tx_info, ctx| {
                    let trace = TransactionOpcodeGas {
                        transaction_hash: tx_info.hash.expect("tx hash is set"),
                        opcode_gas: ctx.inspector.opcode_gas_iter().collect(),
                    };
                    Ok(trace)
                },
            )
            .await?
        else {
            return Ok(None);
        };

        Ok(Some(BlockOpcodeGas {
            block_hash: block.hash(),
            block_number: block.number(),
            transactions,
        }))
    }

    /// Returns all storage slots accessed during transaction execution along with their access
    /// counts.
    pub async fn trace_block_storage_access(
        &self,
        block_id: BlockId,
    ) -> Result<Option<BlockStorageAccess>, Eth::Error> {
        let Some(block) = self.eth_api().recovered_block(block_id).await? else {
            return Err(EthApiError::HeaderNotFound(block_id).into());
        };

        let Some(transactions) = self
            .eth_api()
            .trace_block_inspector(
                block_id,
                Some(block.clone()),
                StorageInspector::default,
                move |tx_info, mut ctx| {
                    let unique_loads = ctx.inspector.unique_loads();
                    let warm_loads = ctx.inspector.warm_loads();
                    let trace = TransactionStorageAccess {
                        transaction_hash: tx_info.hash.expect("tx hash is set"),
                        storage_access: ctx.take_inspector().into_accessed_slots(),
                        unique_loads,
                        warm_loads,
                    };
                    Ok(trace)
                },
            )
            .await?
        else {
            return Ok(None);
        };

        Ok(Some(BlockStorageAccess {
            block_hash: block.hash(),
            block_number: block.number(),
            transactions,
        }))
    }
}

fn apply_trace_filter_pagination(
    all_traces: &mut Vec<LocalizedTransactionTrace>,
    after: &mut Option<u64>,
    count: Option<u64>,
) -> Option<Vec<LocalizedTransactionTrace>> {
    // Skips the first `after` number of matching traces.
    if let Some(cutoff) = after.map(|a| a as usize) &&
        cutoff < all_traces.len()
    {
        all_traces.drain(..cutoff);
        // we removed the first `after` traces
        *after = None;
    }

    // Return at most `count` traces after `after` has been consumed.
    if after.is_none() &&
        let Some(count) = count
    {
        let count = count as usize;
        if count < all_traces.len() {
            all_traces.truncate(count);
            return Some(std::mem::take(all_traces))
        }
    }

    None
}

#[async_trait]
impl<Eth> TraceApiServer<RpcTxReq<Eth::NetworkTypes>> for TraceApi<Eth>
where
    Eth: TraceExt + 'static,
    TxEnvFor<Eth::Evm>: AsRef<RecoveredTxEnvelope>,
    ProviderTx<Eth::Provider>: Clone,
{
    /// Executes the given call and returns a number of possible traces for it.
    ///
    /// Handler for `trace_call`
    async fn trace_call(
        &self,
        call: RpcTxReq<Eth::NetworkTypes>,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<TraceResults> {
        let _ = (call, trace_types, block_id, state_overrides, block_overrides);
        unsupported_trace()
    }

    /// Handler for `trace_callMany`
    async fn trace_call_many(
        &self,
        calls: Vec<(RpcTxReq<Eth::NetworkTypes>, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> RpcResult<Vec<TraceResults>> {
        let _ = (calls, block_id);
        unsupported_trace()
    }

    /// Handler for `trace_rawTransaction`
    async fn trace_raw_transaction(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> RpcResult<TraceResults> {
        let _ = (data, trace_types, block_id);
        unsupported_trace()
    }

    /// Handler for `trace_replayBlockTransactions`
    async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> RpcResult<Option<Vec<TraceResultsWithTransactionHash>>> {
        self.eth_api()
            .trace_block_with(
                block_id,
                None,
                TracingInspectorConfig::from_parity_config(&trace_types),
                move |tx_info, ctx| {
                    let TracingCtx { result, db, inspector } = ctx;
                    let mut db = Db::new(db.clone());
                    let full_trace = inspector
                        .into_parity_builder()
                        .into_trace_results_with_state(&result, &trace_types, &mut db)
                        .map_err(|err| {
                            Eth::Error::from_eth_err(EthApiError::EvmCustom(format!(
                                "EVM database error: {err:?}"
                            )))
                        })?;

                    Ok(TraceResultsWithTransactionHash {
                        transaction_hash: tx_info.hash.expect("tx hash is set"),
                        full_trace,
                    })
                },
            )
            .await
            .map_err(Into::into)
    }

    /// Handler for `trace_replayTransaction`
    async fn replay_transaction(
        &self,
        transaction: B256,
        trace_types: HashSet<TraceType>,
    ) -> RpcResult<TraceResults> {
        let config = TracingInspectorConfig::from_parity_config(&trace_types);
        self.eth_api()
            .spawn_trace_transaction_in_block(transaction, config, move |_, inspector, res, db| {
                let mut db = Db::new(db);
                inspector
                    .into_parity_builder()
                    .into_trace_results_with_state(&res, &trace_types, &mut db)
                    .map_err(|err| {
                        Eth::Error::from_eth_err(EthApiError::EvmCustom(format!(
                            "EVM database error: {err:?}"
                        )))
                    })
            })
            .await
            .transpose()
            .ok_or(EthApiError::TransactionNotFound)?
            .map_err(Into::into)
    }

    /// Handler for `trace_block`
    async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>> {
        let Some(block) = self.eth_api().recovered_block(block_id).await.map_err(Into::into)?
        else {
            return Err(EthApiError::HeaderNotFound(block_id).into());
        };

        let mut traces = self
            .eth_api()
            .trace_block_with(
                block_id,
                Some(block.clone()),
                TracingInspectorConfig::default_parity(),
                |tx_info, ctx| {
                    Ok(ctx
                        .take_inspector()
                        .into_parity_builder()
                        .into_localized_transaction_traces(tx_info))
                },
            )
            .await
            .map_err(Into::into)?
            .map(|traces| traces.into_iter().flatten().collect::<Vec<_>>());

        if let Some(traces) = traces.as_mut() &&
            let Some(base_block_reward) = calculate_base_block_reward(
                self.eth_api().provider().chain_spec().as_ref(),
                block.number(),
            )
        {
            traces.extend(extract_reward_traces(
                block.hash(),
                block.header(),
                block.body().ommers(),
                base_block_reward,
            ));
        }

        Ok(traces)
    }

    /// Handler for `trace_filter`
    ///
    /// This is similar to `eth_getLogs` but for traces.
    ///
    /// # Limitations
    /// This currently requires block filter fields, since reth does not have address indices yet.
    async fn trace_filter(&self, filter: TraceFilter) -> RpcResult<Vec<LocalizedTransactionTrace>> {
        let matcher = Arc::new(filter.matcher());
        let TraceFilter { from_block, to_block, mut after, count, .. } = filter;
        let start = from_block.unwrap_or(0);

        let latest_block =
            self.eth_api().provider().best_block_number().map_err(EthApiError::from)?;
        if start > latest_block {
            return Err(EthApiError::HeaderNotFound(start.into()).into());
        }
        let end = to_block.unwrap_or(latest_block);
        if end > latest_block {
            return Err(EthApiError::HeaderNotFound(end.into()).into());
        }

        let earliest_block =
            self.eth_api().provider().earliest_block_number().map_err(EthApiError::from)?;
        if start < earliest_block {
            return Err(EthApiError::PrunedHistoryUnavailable.into());
        }

        if start > end {
            return Err(EthApiError::InvalidParams(
                "invalid parameters: fromBlock cannot be greater than toBlock".to_string(),
            )
            .into());
        }

        let distance = end.saturating_sub(start);
        if distance > self.inner.eth_config.max_trace_filter_blocks {
            return Err(EthApiError::InvalidParams(format!(
                "Block range too large; currently limited to {} blocks",
                self.inner.eth_config.max_trace_filter_blocks
            ))
            .into());
        }

        let mut all_traces = Vec::new();
        let block_buffer_size =
            self.inner.eth_config.max_tracing_requests.clamp(1, TRACE_FILTER_BLOCK_BUFFER_SIZE);
        let mut include_reward_traces = true;

        for chunk_start in (start..=end).step_by(TRACE_FILTER_FETCH_CHUNK_SIZE) {
            let chunk_end = (chunk_start + TRACE_FILTER_FETCH_CHUNK_SIZE as u64 - 1).min(end);

            let blocks = self
                .eth_api()
                .spawn_blocking_io(move |this| {
                    Ok(this
                        .provider()
                        .recovered_block_range(chunk_start..=chunk_end)
                        .map_err(Eth::Error::from_eth_err)?
                        .into_iter()
                        .map(Arc::new)
                        .collect::<Vec<_>>())
                })
                .await
                .map_err(Into::into)?;

            let mut block_replays = futures::stream::iter(blocks)
                .map(|block| {
                    let this = self.clone();
                    let matcher = matcher.clone();

                    async move {
                        let traces = this
                            .eth_api()
                            .trace_block_with(
                                block.hash().into(),
                                Some(block.clone()),
                                TracingInspectorConfig::default_parity(),
                                move |tx_info, ctx| {
                                    let mut traces = ctx
                                        .take_inspector()
                                        .into_parity_builder()
                                        .into_localized_transaction_traces(tx_info);
                                    traces.retain(|trace| matcher.matches(&trace.trace));
                                    Ok(Some(traces))
                                },
                            )
                            .await?;

                        Ok::<_, Eth::Error>((block, traces))
                    }
                })
                .buffered(block_buffer_size);

            while let Some(block_replay) = block_replays.next().await {
                let (block, traces) = block_replay.map_err(Into::into)?;
                if let Some(traces) = traces {
                    all_traces.extend(traces.into_iter().flatten().flatten());
                }

                let reward_traces = if include_reward_traces {
                    if let Some(base_block_reward) = calculate_base_block_reward(
                        self.eth_api().provider().chain_spec().as_ref(),
                        block.number(),
                    ) {
                        extract_reward_traces(
                            block.hash(),
                            block.header(),
                            block.body().ommers(),
                            base_block_reward,
                        )
                        .into_iter()
                        .filter(|trace| matcher.matches(&trace.trace))
                        .collect::<Vec<_>>()
                    } else {
                        // Blocks are processed in ascending order, so once a historical range
                        // reaches post-Paris blocks, later blocks in the range have no rewards.
                        include_reward_traces = false;
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                all_traces.extend(reward_traces);

                if let Some(traces) =
                    apply_trace_filter_pagination(&mut all_traces, &mut after, count)
                {
                    return Ok(traces)
                }
            }
        }

        if let Some(cutoff) = after.map(|a| a as usize) &&
            cutoff >= all_traces.len()
        {
            return Ok(vec![])
        }

        Ok(all_traces)
    }

    /// Returns transaction trace at given index.
    /// Handler for `trace_get`
    async fn trace_get(
        &self,
        hash: B256,
        indices: Vec<Index>,
    ) -> RpcResult<Option<LocalizedTransactionTrace>> {
        if indices.len() != 1 {
            return Ok(None)
        }

        let index = usize::from(indices[0]);
        Ok(self.trace_transaction(hash).await?.and_then(|traces| traces.into_iter().nth(index)))
    }

    /// Handler for `trace_transaction`
    async fn trace_transaction(
        &self,
        hash: B256,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>> {
        self.eth_api()
            .spawn_trace_transaction_in_block(
                hash,
                TracingInspectorConfig::default_parity(),
                move |tx_info, inspector, _, _| {
                    Ok(inspector.into_parity_builder().into_localized_transaction_traces(tx_info))
                },
            )
            .await
            .map_err(Into::into)
    }

    /// Handler for `trace_transactionOpcodeGas`
    async fn trace_transaction_opcode_gas(
        &self,
        tx_hash: B256,
    ) -> RpcResult<Option<TransactionOpcodeGas>> {
        self.eth_api()
            .spawn_trace_transaction_in_block_with_inspector(
                tx_hash,
                OpcodeGasInspector::default(),
                move |_tx_info, inspector, _res, _| {
                    Ok(TransactionOpcodeGas {
                        transaction_hash: tx_hash,
                        opcode_gas: inspector.opcode_gas_iter().collect(),
                    })
                },
            )
            .await
            .map_err(Into::into)
    }

    /// Handler for `trace_blockOpcodeGas`
    async fn trace_block_opcode_gas(&self, block_id: BlockId) -> RpcResult<Option<BlockOpcodeGas>> {
        let Some(block) = self.eth_api().recovered_block(block_id).await.map_err(Into::into)?
        else {
            return Err(EthApiError::HeaderNotFound(block_id).into());
        };

        let Some(transactions) = self
            .eth_api()
            .trace_block_inspector(
                block_id,
                Some(block.clone()),
                OpcodeGasInspector::default,
                move |tx_info, ctx| {
                    Ok(TransactionOpcodeGas {
                        transaction_hash: tx_info.hash.expect("tx hash is set"),
                        opcode_gas: ctx.inspector.opcode_gas_iter().collect(),
                    })
                },
            )
            .await
            .map_err(Into::into)?
        else {
            return Ok(None);
        };

        Ok(Some(BlockOpcodeGas {
            block_hash: block.hash(),
            block_number: block.number(),
            transactions,
        }))
    }
}

impl<Eth> std::fmt::Debug for TraceApi<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceApi").finish_non_exhaustive()
    }
}
impl<Eth> Clone for TraceApi<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct TraceApiInner<Eth> {
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    // restrict the number of concurrent calls to `trace_*`
    #[expect(dead_code)]
    blocking_task_guard: BlockingTaskGuard,
    // eth config settings
    eth_config: EthConfig,
}

fn calculate_base_block_reward<C>(chain_spec: &C, block_number: u64) -> Option<u128>
where
    C: EthereumHardforks,
{
    if chain_spec.is_paris_active_at_block(block_number) {
        None
    } else if chain_spec.is_constantinople_active_at_block(block_number) {
        Some(2 * ETH_TO_WEI)
    } else if chain_spec.is_byzantium_active_at_block(block_number) {
        Some(3 * ETH_TO_WEI)
    } else {
        Some(5 * ETH_TO_WEI)
    }
}

const fn block_reward(base_block_reward: u128, ommers: usize) -> u128 {
    base_block_reward + (base_block_reward >> 5) * ommers as u128
}

fn ommer_reward(base_block_reward: u128, block_number: u64, ommer_block_number: u64) -> u128 {
    let distance = 8 + ommer_block_number - block_number;
    (u128::from(distance) * base_block_reward) >> 3
}

fn extract_reward_traces<H: BlockHeader>(
    block_hash: BlockHash,
    header: &H,
    ommers: Option<&[H]>,
    base_block_reward: u128,
) -> Vec<LocalizedTransactionTrace> {
    let ommers_cnt = ommers.map(|o| o.len()).unwrap_or_default();
    let mut traces = Vec::with_capacity(ommers_cnt + 1);

    traces.push(reward_trace(
        block_hash,
        header,
        RewardAction {
            author: header.beneficiary(),
            reward_type: RewardType::Block,
            value: U256::from(block_reward(base_block_reward, ommers_cnt)),
        },
    ));

    let Some(ommers) = ommers else { return traces };

    for ommer in ommers {
        traces.push(reward_trace(
            block_hash,
            header,
            RewardAction {
                author: ommer.beneficiary(),
                reward_type: RewardType::Uncle,
                value: U256::from(ommer_reward(base_block_reward, header.number(), ommer.number())),
            },
        ));
    }

    traces
}

fn reward_trace<H: BlockHeader>(
    block_hash: BlockHash,
    header: &H,
    reward: RewardAction,
) -> LocalizedTransactionTrace {
    LocalizedTransactionTrace {
        block_hash: Some(block_hash),
        block_number: Some(header.number()),
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

/// Response type for storage tracing that contains all accessed storage slots
/// for a transaction.
#[cfg(any())]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionStorageAccess {
    /// Hash of the transaction
    pub transaction_hash: B256,
    /// Tracks storage slots and access counter.
    pub storage_access: HashMap<Address, HashMap<B256, u64>>,
    /// Number of unique storage loads
    pub unique_loads: u64,
    /// Number of warm storage loads
    pub warm_loads: u64,
}

/// Response type for storage tracing that contains all accessed storage slots
#[cfg(any())]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockStorageAccess {
    /// The block hash
    pub block_hash: BlockHash,
    /// The block's number
    pub block_number: u64,
    /// All executed transactions in the block in the order they were executed
    pub transactions: Vec<TransactionStorageAccess>,
}

/// Helper to construct a [`LocalizedTransactionTrace`] that describes a reward to the block
/// beneficiary.
#[cfg(any())]
fn reward_trace<H: BlockHeader>(
    block_hash: BlockHash,
    header: &H,
    reward: RewardAction,
) -> LocalizedTransactionTrace {
    LocalizedTransactionTrace {
        block_hash: Some(block_hash),
        block_number: Some(header.number()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

    fn localized_transaction_trace(
        block_number: u64,
        transaction_position: u64,
    ) -> LocalizedTransactionTrace {
        LocalizedTransactionTrace {
            block_hash: Some(B256::ZERO),
            block_number: Some(block_number),
            transaction_hash: Some(B256::ZERO),
            transaction_position: Some(transaction_position),
            trace: TransactionTrace::default(),
        }
    }

    fn localized_reward_trace(block_number: u64) -> LocalizedTransactionTrace {
        LocalizedTransactionTrace {
            block_hash: Some(B256::ZERO),
            block_number: Some(block_number),
            transaction_hash: None,
            transaction_position: None,
            trace: TransactionTrace {
                trace_address: vec![],
                subtraces: 0,
                action: Action::Reward(RewardAction {
                    author: Address::ZERO,
                    reward_type: RewardType::Block,
                    value: U256::ZERO,
                }),
                error: None,
                result: None,
            },
        }
    }

    fn trace_order(traces: &[LocalizedTransactionTrace]) -> Vec<(u64, Option<u64>, bool)> {
        traces
            .iter()
            .map(|trace| {
                (
                    trace.block_number.unwrap(),
                    trace.transaction_position,
                    trace.trace.action.is_reward(),
                )
            })
            .collect()
    }

    #[test]
    fn trace_filter_paginates_after_per_block_reward_order() {
        let mut all_traces = vec![
            localized_transaction_trace(1, 0),
            localized_reward_trace(1),
            localized_transaction_trace(2, 0),
            localized_reward_trace(2),
        ];

        let mut after = Some(1);
        let paginated =
            apply_trace_filter_pagination(&mut all_traces, &mut after, Some(1)).unwrap();

        assert_eq!(trace_order(&paginated), vec![(1, None, true)]);
    }
}
