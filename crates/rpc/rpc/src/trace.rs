use std::sync::Arc;

use alloy_primitives::{map::HashSet, Bytes, B256, U256};
use alloy_rpc_types::{
    state::{EvmOverrides, StateOverride},
    BlockOverrides, Index,
};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use alloy_rpc_types_trace::{
    filter::TraceFilter,
    opcode::{BlockOpcodeGas, TransactionOpcodeGas},
    parity::*,
    tracerequest::TraceCallRequest,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::EthereumHardforks;
use reth_consensus_common::calc::{
    base_block_reward, base_block_reward_pre_merge, block_reward, ommer_reward,
};
use reth_evm::ConfigureEvmEnv;
use reth_primitives::{BlockId, Header};
use reth_provider::{BlockReader, ChainSpecProvider, EvmEnvProvider, StateProviderFactory};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_api::TraceApiServer;
use reth_rpc_eth_api::{
    helpers::{Call, TraceExt},
    FromEthApiError,
};
use reth_rpc_eth_types::{error::EthApiError, utils::recover_raw_transaction};
use reth_tasks::pool::BlockingTaskGuard;
use revm::{
    db::{CacheDB, DatabaseCommit},
    primitives::EnvWithHandlerCfg,
};
use revm_inspectors::{
    opcode::OpcodeGasInspector,
    tracing::{parity::populate_state_diff, TracingInspector, TracingInspectorConfig},
};
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

    /// Create a new instance of the [`TraceApi`]
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

    /// Access the underlying `Eth` API.
    pub fn eth_api(&self) -> &Eth {
        &self.inner.eth_api
    }
}

// === impl TraceApi ===

impl<Provider, Eth> TraceApi<Provider, Eth>
where
    Provider: BlockReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + 'static,
    Eth: TraceExt + 'static,
{
    /// Executes the given call and returns a number of possible traces for it.
    pub async fn trace_call(
        &self,
        trace_request: TraceCallRequest,
    ) -> Result<TraceResults, Eth::Error> {
        let at = trace_request.block_id.unwrap_or_default();
        let config = TracingInspectorConfig::from_parity_config(&trace_request.trace_types);
        let overrides =
            EvmOverrides::new(trace_request.state_overrides, trace_request.block_overrides);
        let mut inspector = TracingInspector::new(config);
        let this = self.clone();
        self.eth_api()
            .spawn_with_call_at(trace_request.call, at, overrides, move |db, env| {
                // wrapper is hack to get around 'higher-ranked lifetime error', see
                // <https://github.com/rust-lang/rust/issues/100013>
                let db = db.0;

                let (res, _) = this.eth_api().inspect(&mut *db, env, &mut inspector)?;
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
        let tx = recover_raw_transaction(tx)?;

        let (cfg, block, at) = self.eth_api().evm_env_at(block_id.unwrap_or_default()).await?;

        let env = EnvWithHandlerCfg::new_with_cfg_env(
            cfg,
            block,
            Call::evm_config(self.eth_api()).tx_env(&tx.into_ecrecovered_transaction()),
        );

        let config = TracingInspectorConfig::from_parity_config(&trace_types);

        self.eth_api()
            .spawn_trace_at_with_state(env, config, at, move |inspector, res, db| {
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
        calls: Vec<(TransactionRequest, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> Result<Vec<TraceResults>, Eth::Error> {
        let at = block_id.unwrap_or(BlockId::pending());
        let (cfg, block_env, at) = self.eth_api().evm_env_at(at).await?;

        let this = self.clone();
        // execute all transactions on top of each other and record the traces
        self.eth_api()
            .spawn_with_state_at_block(at, move |state| {
                let mut results = Vec::with_capacity(calls.len());
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                let mut calls = calls.into_iter().peekable();

                while let Some((call, trace_types)) = calls.next() {
                    let env = this.eth_api().prepare_call_env(
                        cfg.clone(),
                        block_env.clone(),
                        call,
                        &mut db,
                        Default::default(),
                    )?;
                    let config = TracingInspectorConfig::from_parity_config(&trace_types);
                    let mut inspector = TracingInspector::new(config);
                    let (res, _) = this.eth_api().inspect(&mut db, env, &mut inspector)?;

                    let trace_res = inspector
                        .into_parity_builder()
                        .into_trace_results_with_state(&res, &trace_types, &db)
                        .map_err(Eth::Error::from_eth_err)?;

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

    /// Returns all transaction traces that match the given filter.
    ///
    /// This is similar to [`Self::trace_block`] but only returns traces for transactions that match
    /// the filter.
    pub async fn trace_filter(
        &self,
        filter: TraceFilter,
    ) -> Result<Vec<LocalizedTransactionTrace>, Eth::Error> {
        let matcher = filter.matcher();
        let TraceFilter { from_block, to_block, after, count, .. } = filter;
        let start = from_block.unwrap_or(0);
        let end = if let Some(to_block) = to_block {
            to_block
        } else {
            self.provider().best_block_number().map_err(Eth::Error::from_eth_err)?
        };

        if start > end {
            return Err(EthApiError::InvalidParams(
                "invalid parameters: fromBlock cannot be greater than toBlock".to_string(),
            )
            .into())
        }

        // ensure that the range is not too large, since we need to fetch all blocks in the range
        let distance = end.saturating_sub(start);
        if distance > 100 {
            return Err(EthApiError::InvalidParams(
                "Block range too large; currently limited to 100 blocks".to_string(),
            )
            .into())
        }

        // fetch all blocks in that range
        let blocks = self.provider().block_range(start..=end).map_err(Eth::Error::from_eth_err)?;

        // trace all blocks
        let mut block_traces = Vec::with_capacity(blocks.len());
        for block in &blocks {
            let matcher = matcher.clone();
            let traces = self.eth_api().trace_block_until(
                block.number.into(),
                None,
                TracingInspectorConfig::default_parity(),
                move |tx_info, inspector, _, _, _| {
                    let mut traces =
                        inspector.into_parity_builder().into_localized_transaction_traces(tx_info);
                    traces.retain(|trace| matcher.matches(&trace.trace));
                    Ok(Some(traces))
                },
            );
            block_traces.push(traces);
        }

        let block_traces = futures::future::try_join_all(block_traces).await?;
        let mut all_traces = block_traces
            .into_iter()
            .flatten()
            .flat_map(|traces| traces.into_iter().flatten().flat_map(|traces| traces.into_iter()))
            .collect::<Vec<_>>();

        // add reward traces for all blocks
        for block in &blocks {
            if let Some(base_block_reward) = self.calculate_base_block_reward(&block.header)? {
                let mut traces = self.extract_reward_traces(
                    &block.header,
                    &block.body.ommers,
                    base_block_reward,
                );
                traces.retain(|trace| matcher.matches(&trace.trace));
                all_traces.extend(traces);
            } else {
                // no block reward, means we're past the Paris hardfork and don't expect any rewards
                // because the blocks in ascending order
                break
            }
        }

        // apply after and count to traces if specified, this allows for a pagination style.
        // only consider traces after
        if let Some(after) = after.map(|a| a as usize).filter(|a| *a < all_traces.len()) {
            all_traces = all_traces.split_off(after);
        }

        // at most, return count of traces
        if let Some(count) = count {
            let count = count as usize;
            if count < all_traces.len() {
                all_traces.truncate(count);
            }
        };

        Ok(all_traces)
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

    /// Returns traces created at given block.
    pub async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>, Eth::Error> {
        let traces = self.eth_api().trace_block_with(
            block_id,
            TracingInspectorConfig::default_parity(),
            |tx_info, inspector, _, _, _| {
                let traces =
                    inspector.into_parity_builder().into_localized_transaction_traces(tx_info);
                Ok(traces)
            },
        );

        let block = self.eth_api().block(block_id);
        let (maybe_traces, maybe_block) = futures::try_join!(traces, block)?;

        let mut maybe_traces =
            maybe_traces.map(|traces| traces.into_iter().flatten().collect::<Vec<_>>());

        if let (Some(block), Some(traces)) = (maybe_block, maybe_traces.as_mut()) {
            if let Some(base_block_reward) = self.calculate_base_block_reward(&block.header)? {
                traces.extend(self.extract_reward_traces(
                    &block.header,
                    &block.body.ommers,
                    base_block_reward,
                ));
            }
        }

        Ok(maybe_traces)
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
                TracingInspectorConfig::from_parity_config(&trace_types),
                move |tx_info, inspector, res, state, db| {
                    let mut full_trace =
                        inspector.into_parity_builder().into_trace_results(&res, &trace_types);

                    // If statediffs were requested, populate them with the account balance and
                    // nonce from pre-state
                    if let Some(ref mut state_diff) = full_trace.state_diff {
                        populate_state_diff(state_diff, db, state.iter())
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

    /// Returns the opcodes of all transactions in the given block.
    ///
    /// This is the same as [`Self::trace_transaction_opcode_gas`] but for all transactions in a
    /// block.
    pub async fn trace_block_opcode_gas(
        &self,
        block_id: BlockId,
    ) -> Result<Option<BlockOpcodeGas>, Eth::Error> {
        let res = self
            .eth_api()
            .trace_block_inspector(
                block_id,
                OpcodeGasInspector::default,
                move |tx_info, inspector, _res, _, _| {
                    let trace = TransactionOpcodeGas {
                        transaction_hash: tx_info.hash.expect("tx hash is set"),
                        opcode_gas: inspector.opcode_gas_iter().collect(),
                    };
                    Ok(trace)
                },
            )
            .await?;

        let Some(transactions) = res else { return Ok(None) };

        let Some(block) = self.eth_api().block(block_id).await? else { return Ok(None) };

        Ok(Some(BlockOpcodeGas {
            block_hash: block.hash(),
            block_number: block.header.number,
            transactions,
        }))
    }

    /// Calculates the base block reward for the given block:
    ///
    /// - if Paris hardfork is activated, no block rewards are given
    /// - if Paris hardfork is not activated, calculate block rewards with block number only
    /// - if Paris hardfork is unknown, calculate block rewards with block number and ttd
    fn calculate_base_block_reward(&self, header: &Header) -> Result<Option<u128>, Eth::Error> {
        let chain_spec = self.provider().chain_spec();
        let is_paris_activated = chain_spec.is_paris_active_at_block(header.number);

        Ok(match is_paris_activated {
            Some(true) => None,
            Some(false) => Some(base_block_reward_pre_merge(&chain_spec, header.number)),
            None => {
                // if Paris hardfork is unknown, we need to fetch the total difficulty at the
                // block's height and check if it is pre-merge to calculate the base block reward
                if let Some(header_td) = self
                    .provider()
                    .header_td_by_number(header.number)
                    .map_err(Eth::Error::from_eth_err)?
                {
                    base_block_reward(
                        chain_spec.as_ref(),
                        header.number,
                        header.difficulty,
                        header_td,
                    )
                } else {
                    None
                }
            }
        })
    }

    /// Extracts the reward traces for the given block:
    ///  - block reward
    ///  - uncle rewards
    fn extract_reward_traces(
        &self,
        header: &Header,
        ommers: &[Header],
        base_block_reward: u128,
    ) -> Vec<LocalizedTransactionTrace> {
        let mut traces = Vec::with_capacity(ommers.len() + 1);

        let block_reward = block_reward(base_block_reward, ommers.len());
        traces.push(reward_trace(
            header,
            RewardAction {
                author: header.beneficiary,
                reward_type: RewardType::Block,
                value: U256::from(block_reward),
            },
        ));

        for uncle in ommers {
            let uncle_reward = ommer_reward(base_block_reward, header.number, uncle.number);
            traces.push(reward_trace(
                header,
                RewardAction {
                    author: uncle.beneficiary,
                    reward_type: RewardType::Uncle,
                    value: U256::from(uncle_reward),
                },
            ));
        }
        traces
    }
}

#[async_trait]
impl<Provider, Eth> TraceApiServer for TraceApi<Provider, Eth>
where
    Provider: BlockReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + 'static,
    Eth: TraceExt + 'static,
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
    ) -> RpcResult<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        let request =
            TraceCallRequest { call, trace_types, block_id, state_overrides, block_overrides };
        Ok(Self::trace_call(self, request).await.map_err(Into::into)?)
    }

    /// Handler for `trace_callMany`
    async fn trace_call_many(
        &self,
        calls: Vec<(TransactionRequest, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> RpcResult<Vec<TraceResults>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::trace_call_many(self, calls, block_id).await.map_err(Into::into)?)
    }

    /// Handler for `trace_rawTransaction`
    async fn trace_raw_transaction(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> RpcResult<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::trace_raw_transaction(self, data, trace_types, block_id)
            .await
            .map_err(Into::into)?)
    }

    /// Handler for `trace_replayBlockTransactions`
    async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> RpcResult<Option<Vec<TraceResultsWithTransactionHash>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::replay_block_transactions(self, block_id, trace_types)
            .await
            .map_err(Into::into)?)
    }

    /// Handler for `trace_replayTransaction`
    async fn replay_transaction(
        &self,
        transaction: B256,
        trace_types: HashSet<TraceType>,
    ) -> RpcResult<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::replay_transaction(self, transaction, trace_types).await.map_err(Into::into)?)
    }

    /// Handler for `trace_block`
    async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::trace_block(self, block_id).await.map_err(Into::into)?)
    }

    /// Handler for `trace_filter`
    ///
    /// This is similar to `eth_getLogs` but for traces.
    ///
    /// # Limitations
    /// This currently requires block filter fields, since reth does not have address indices yet.
    async fn trace_filter(&self, filter: TraceFilter) -> RpcResult<Vec<LocalizedTransactionTrace>> {
        Ok(Self::trace_filter(self, filter).await.map_err(Into::into)?)
    }

    /// Returns transaction trace at given index.
    /// Handler for `trace_get`
    async fn trace_get(
        &self,
        hash: B256,
        indices: Vec<Index>,
    ) -> RpcResult<Option<LocalizedTransactionTrace>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::trace_get(self, hash, indices.into_iter().map(Into::into).collect())
            .await
            .map_err(Into::into)?)
    }

    /// Handler for `trace_transaction`
    async fn trace_transaction(
        &self,
        hash: B256,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::trace_transaction(self, hash).await.map_err(Into::into)?)
    }

    /// Handler for `trace_transactionOpcodeGas`
    async fn trace_transaction_opcode_gas(
        &self,
        tx_hash: B256,
    ) -> RpcResult<Option<TransactionOpcodeGas>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::trace_transaction_opcode_gas(self, tx_hash).await.map_err(Into::into)?)
    }

    /// Handler for `trace_blockOpcodeGas`
    async fn trace_block_opcode_gas(&self, block_id: BlockId) -> RpcResult<Option<BlockOpcodeGas>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(Self::trace_block_opcode_gas(self, block_id).await.map_err(Into::into)?)
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
fn reward_trace(header: &Header, reward: RewardAction) -> LocalizedTransactionTrace {
    LocalizedTransactionTrace {
        block_hash: Some(header.hash_slow()),
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
