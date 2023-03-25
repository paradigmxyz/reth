use crate::{
    eth::{
        cache::EthStateCache, error::EthResult, revm_utils::inspect,
        utils::recover_raw_transaction, EthTransactions,
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
    trace::{filter::TraceFilter, parity::*},
    CallRequest, Index,
};
use revm::primitives::{Env, ExecutionResult, ResultAndState};
use std::collections::HashSet;
use tokio::sync::{AcquireError, OwnedSemaphorePermit};

/// `trace` API implementation.
///
/// This type provides the functionality for handling `trace` related requests.
#[derive(Clone)]
pub struct TraceApi<Client, Eth> {
    /// The client that can interact with the chain.
    client: Client,
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,

    // restrict the number of concurrent calls to `trace_*`
    tracing_call_guard: TracingCallGuard,
}

// === impl TraceApi ===

impl<Client, Eth> TraceApi<Client, Eth> {
    /// Create a new instance of the [TraceApi]
    pub fn new(
        client: Client,
        eth_api: Eth,
        eth_cache: EthStateCache,
        tracing_call_guard: TracingCallGuard,
    ) -> Self {
        Self { client, eth_api, eth_cache, tracing_call_guard }
    }

    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(
        &self,
    ) -> std::result::Result<OwnedSemaphorePermit, AcquireError> {
        self.tracing_call_guard.clone().acquire_owned().await
    }
}

// === impl TraceApi ===

impl<Client, Eth> TraceApi<Client, Eth>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Executes the transaction at the given [BlockId] with a tracer configured by the config.
    fn trace_at<F, R>(
        &self,
        env: Env,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        F: FnOnce(TracingInspector, ResultAndState) -> EthResult<R>,
    {
        self.eth_api.with_state_at(at, |state| {
            let db = SubState::new(State::new(state));

            let mut inspector = TracingInspector::new(config);
            let (res, _) = inspect(db, env, &mut inspector)?;

            f(inspector, res)
        })
    }

    /// Executes the given call and returns a number of possible traces for it.
    pub async fn trace_call(
        &self,
        _call: CallRequest,
        _trace_types: HashSet<TraceType>,
        _block_id: Option<BlockId>,
    ) -> EthResult<TraceResults> {
        todo!()
    }

    /// Traces a call to `eth_sendRawTransaction` without making the call, returning the traces.
    pub async fn trace_raw_transaction(
        &self,
        tx: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> EthResult<TraceResults> {
        let _permit = self.acquire_trace_permit().await;
        let tx = recover_raw_transaction(tx)?;

        let (cfg, block, at) = self
            .eth_api
            .evm_env_at(block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest)))
            .await?;
        let tx = tx_env_with_recovered(&tx);
        let env = Env { cfg, block, tx };

        let config = tracing_config(&trace_types);

        self.trace_at(env, config, at, |inspector, res| {
            let output = match res.result {
                ExecutionResult::Success { output, .. } => output.into_data(),
                ExecutionResult::Revert { output, .. } => output,
                ExecutionResult::Halt { .. } => Default::default(),
            };

            let (trace, vm_trace, state_diff) =
                inspector.into_parity_builder().into_trace_type_traces(&trace_types);

            let res = TraceResults { output: output.into(), trace, vm_trace, state_diff };

            Ok(res)
        })
    }

    /// Returns transaction trace with the given address.
    pub async fn trace_get(
        &self,
        hash: H256,
        trace_address: Vec<usize>,
    ) -> EthResult<Option<LocalizedTransactionTrace>> {
        let _permit = self.acquire_trace_permit().await;

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
        let _permit = self.acquire_trace_permit().await;

        let (transaction, at) = match self.eth_api.transaction_by_hash_at(hash).await? {
            None => return Ok(None),
            Some(res) => res,
        };

        let (cfg, block, at) = self.eth_api.evm_env_at(at).await?;

        let (tx, tx_info) = transaction.split();
        let tx = tx_env_with_recovered(&tx);
        let env = Env { cfg, block, tx };

        // execute the trace
        self.trace_at(env, TracingInspectorConfig::default_parity(), at, |inspector, _| {
            let traces = inspector.into_parity_builder().into_localized_transaction_traces(tx_info);
            Ok(Some(traces))
        })
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
    ) -> Result<TraceResults> {
        Ok(TraceApi::trace_call(self, call, trace_types, block_id).await?)
    }

    /// Handler for `trace_callMany`
    async fn trace_call_many(
        &self,
        _calls: Vec<(CallRequest, HashSet<TraceType>)>,
        _block_id: Option<BlockId>,
    ) -> Result<Vec<TraceResults>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `trace_rawTransaction`
    async fn trace_raw_transaction(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> Result<TraceResults> {
        Ok(TraceApi::trace_raw_transaction(self, data, trace_types, block_id).await?)
    }

    /// Handler for `trace_replayBlockTransactions`
    async fn replay_block_transactions(
        &self,
        _block_id: BlockId,
        _trace_types: HashSet<TraceType>,
    ) -> Result<Option<Vec<TraceResultsWithTransactionHash>>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `trace_replayTransaction`
    async fn replay_transaction(
        &self,
        _transaction: H256,
        _trace_types: HashSet<TraceType>,
    ) -> Result<TraceResults> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `trace_block`
    async fn trace_block(
        &self,
        _block_id: BlockId,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        Err(internal_rpc_err("unimplemented"))
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
        Ok(TraceApi::trace_get(self, hash, indices.into_iter().map(Into::into).collect()).await?)
    }

    /// Handler for `trace_transaction`
    async fn trace_transaction(
        &self,
        hash: H256,
    ) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        Ok(TraceApi::trace_transaction(self, hash).await?)
    }
}

impl<Client, Eth> std::fmt::Debug for TraceApi<Client, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceApi").finish_non_exhaustive()
    }
}

/// Returns the [TracingInspectorConfig] depending on the enabled [TraceType]s
fn tracing_config(trace_types: &HashSet<TraceType>) -> TracingInspectorConfig {
    TracingInspectorConfig::default_parity()
        .set_state_diffs(trace_types.contains(&TraceType::StateDiff))
        .set_steps(trace_types.contains(&TraceType::VmTrace))
}
