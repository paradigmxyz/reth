use crate::{
    eth::{
        error::{EthApiError, EthResult},
        revm_utils::inspect,
        EthTransactions, TransactionSource,
    },
    result::{internal_rpc_err, ToRpcResult},
    EthApiSpec, TracingCallGuard,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{BlockId, BlockNumberOrTag, Bytes, H256, U256};
use reth_provider::{BlockProvider, HeaderProvider};
use reth_revm::{
    database::{State, SubState},
    env::tx_env_with_recovered,
    tracing::{TracingInspector, TracingInspectorConfig},
};
use reth_rpc_api::DebugApiServer;
use reth_rpc_types::{
    trace::geth::{
        BlockTraceResult, GethDebugBuiltInTracerType, GethDebugTracerType, GethDebugTracingOptions,
        GethTraceFrame, NoopFrame, TraceResult,
    },
    CallRequest, RichBlock,
};
use revm::primitives::Env;

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
#[non_exhaustive]
pub struct DebugApi<Client, Eth> {
    /// The client that can interact with the chain.
    client: Client,
    /// The implementation of `eth` API
    eth_api: Eth,

    // restrict the number of concurrent calls to `debug_traceTransaction`
    tracing_call_guard: TracingCallGuard,
}

// === impl DebugApi ===

impl<Client, Eth> DebugApi<Client, Eth> {
    /// Create a new instance of the [DebugApi]
    pub fn new(client: Client, eth: Eth, tracing_call_guard: TracingCallGuard) -> Self {
        Self { client, eth_api: eth, tracing_call_guard }
    }
}

// === impl DebugApi ===

impl<Client, Eth> DebugApi<Client, Eth>
where
    Client: BlockProvider + HeaderProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Trace the transaction according to the provided options.
    ///
    /// Ref: <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>
    pub async fn debug_trace_transaction(
        &self,
        tx_hash: H256,
        opts: GethDebugTracingOptions,
    ) -> EthResult<GethTraceFrame> {
        let (transaction, at) = match self.eth_api.transaction_by_hash_at(tx_hash).await? {
            None => return Err(EthApiError::TransactionNotFound),
            Some(res) => res,
        };

        let (cfg, block, at) = self.eth_api.evm_env_at(at).await?;

        let tx = transaction.into_recovered();

        self.eth_api.with_state_at(at, |state| {
            let tx = tx_env_with_recovered(&tx);
            let env = Env { cfg, block, tx };
            let db = SubState::new(State::new(state));

            let GethDebugTracingOptions { config, tracer, tracer_config, .. } = opts;
            if let Some(tracer) = tracer {
                // valid matching config
                if let Some(ref config) = tracer_config {
                    if !config.matches_tracer(&tracer) {
                        return Err(EthApiError::InvalidTracerConfig)
                    }
                }

                return match tracer {
                    GethDebugTracerType::BuiltInTracer(tracer) => match tracer {
                        GethDebugBuiltInTracerType::FourByteTracer => {
                            todo!()
                        }
                        GethDebugBuiltInTracerType::CallTracer => {
                            todo!()
                        }
                        GethDebugBuiltInTracerType::PreStateTracer => {
                            todo!()
                        }
                        GethDebugBuiltInTracerType::NoopTracer => Ok(NoopFrame::default().into()),
                    },
                    GethDebugTracerType::JsTracer(_) => {
                        Err(EthApiError::Unsupported("javascript tracers are unsupported."))
                    }
                }
            }

            // default structlog tracer
            let inspector_config = TracingInspectorConfig::default_geth()
                .set_memory_snapshots(config.enable_memory.unwrap_or_default())
                .set_stack_snapshots(!config.disable_stack.unwrap_or_default())
                .set_state_diffs(!config.disable_storage.unwrap_or_default());

            let mut inspector = TracingInspector::new(inspector_config);

            let (res, _) = inspect(db, env, &mut inspector)?;
            let gas_used = res.result.gas_used();

            let frame = inspector.into_geth_builder().geth_traces(U256::from(gas_used), config);

            Ok(frame.into())
        })
    }
}

use reth_rlp::Encodable;

#[async_trait]
impl<Client, Eth> DebugApiServer for DebugApi<Client, Eth>
where
    Client: BlockProvider + HeaderProvider + 'static,
    Eth: EthApiSpec + 'static,
{
    /// Handler for `debug_getRawHeader`
    async fn raw_header(&self, block_id: BlockId) -> RpcResult<Bytes> {
        let header = match block_id {
            BlockId::Hash(hash) => self.client.header(&hash.into()).to_rpc_result()?,
            BlockId::Number(number_or_tag) => {
                let number =
                    self.client.convert_block_number(number_or_tag).to_rpc_result()?.ok_or(
                        jsonrpsee::core::Error::Custom("Pending block not supported".to_string()),
                    )?;
                self.client.header_by_number(number).to_rpc_result()?
            }
        };

        let mut res = Vec::new();
        if let Some(header) = header {
            header.encode(&mut res);
        }

        Ok(res.into())
    }

    /// Handler for `debug_getRawBlock`
    async fn raw_block(&self, block_id: BlockId) -> RpcResult<Bytes> {
        let block = self.client.block(block_id).to_rpc_result()?;

        let mut res = Vec::new();
        if let Some(mut block) = block {
            // In RPC withdrawals are always present
            if block.withdrawals.is_none() {
                block.withdrawals = Some(vec![]);
            }
            block.encode(&mut res);
        }

        Ok(res.into())
    }

    /// Handler for `debug_getRawTransaction`
    /// Returns the bytes of the transaction for the given hash.
    async fn raw_transaction(&self, hash: H256) -> RpcResult<Bytes> {
        let tx = self.eth_api.transaction_by_hash(hash).await?;

        let mut res = Vec::new();
        if let Some(tx) = tx.map(TransactionSource::into_recovered) {
            tx.encode(&mut res);
        }

        Ok(res.into())
    }

    /// Handler for `debug_getRawReceipts`
    async fn raw_receipts(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>> {
        let receipts = self.client.receipts_by_block(block_id).to_rpc_result()?.unwrap_or_default();
        let mut all_receipts = Vec::with_capacity(receipts.len());

        for receipt in receipts {
            let mut buf = Vec::new();
            let receipt = receipt.with_bloom();
            receipt.encode(&mut buf);
            all_receipts.push(buf.into());
        }

        Ok(all_receipts)
    }

    /// Handler for `debug_getBadBlocks`
    async fn bad_blocks(&self) -> RpcResult<Vec<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceChain`
    async fn debug_trace_chain(
        &self,
        _start_exclusive: BlockNumberOrTag,
        _end_inclusive: BlockNumberOrTag,
    ) -> RpcResult<Vec<BlockTraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceBlock`
    async fn debug_trace_block(
        &self,
        _rlp_block: Bytes,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceBlockByHash`
    async fn debug_trace_block_by_hash(
        &self,
        _block: H256,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceBlockByNumber`
    async fn debug_trace_block_by_number(
        &self,
        _block: BlockNumberOrTag,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceTransaction`
    async fn debug_trace_transaction(
        &self,
        tx_hash: H256,
        opts: GethDebugTracingOptions,
    ) -> RpcResult<GethTraceFrame> {
        Ok(DebugApi::debug_trace_transaction(self, tx_hash, opts).await?)
    }

    /// Handler for `debug_traceCall`
    async fn debug_trace_call(
        &self,
        _request: CallRequest,
        _block_number: Option<BlockId>,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<GethTraceFrame> {
        Err(internal_rpc_err("unimplemented"))
    }
}

impl<Client, Eth> std::fmt::Debug for DebugApi<Client, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}
