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
use reth_primitives::{Block, BlockId, BlockNumberOrTag, Bytes, H256, U256};
use reth_provider::{BlockProvider, HeaderProvider, StateProviderBox};
use reth_revm::{
    database::{State, SubState},
    env::tx_env_with_recovered,
    tracing::{FourByteInspector, TracingInspector, TracingInspectorConfig},
};
use reth_rlp::{Decodable, Encodable};
use reth_rpc_api::DebugApiServer;
use reth_rpc_types::{
    trace::geth::{
        BlockTraceResult, FourByteFrame, GethDebugBuiltInTracerType, GethDebugTracerType,
        GethDebugTracingCallOptions, GethDebugTracingOptions, GethTraceFrame, NoopFrame,
        TraceResult,
    },
    BlockError, CallRequest, RichBlock,
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
    /// Replays the given block and returns the trace of each transaction.
    ///
    /// This expects a rlp encoded block
    ///
    /// Note, the parent of this block must be present, or it will fail.
    pub async fn debug_trace_raw_block(
        &self,
        rlp_block: Bytes,
        _opts: GethDebugTracingOptions,
    ) -> EthResult<Vec<TraceResult>> {
        let block =
            Block::decode(&mut rlp_block.as_ref()).map_err(BlockError::RlpDecodeRawBlock)?;
        let _parent = block.parent_hash;

        // TODO we need the state after the parent block

        todo!()
    }

    /// Replays a block and returns the trace of each transaction.
    pub async fn debug_trace_block(
        &self,
        block_id: BlockId,
        opts: GethDebugTracingOptions,
    ) -> EthResult<Vec<TraceResult>> {
        let block_hash = self
            .client
            .block_hash_for_id(block_id)?
            .ok_or_else(|| EthApiError::UnknownBlockNumber)?;

        let ((cfg, block_env, at), transactions) = futures::try_join!(
            self.eth_api.evm_env_at(block_hash.into()),
            self.eth_api.transactions_by_block(block_hash),
        )?;
        let transactions = transactions.ok_or_else(|| EthApiError::UnknownBlockNumber)?;

        // replay all transactions of the block
        self.eth_api.with_state_at(at, move |state| {
            let mut results = Vec::with_capacity(transactions.len());
            let mut db = SubState::new(State::new(state));

            for tx in transactions {
                let tx = tx.into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
                let tx = tx_env_with_recovered(&tx);
                let env = Env { cfg: cfg.clone(), block: block_env.clone(), tx };
                // TODO(mattsse): get rid of clone by extracting necessary opts fields into a struct
                let result = trace_transaction(opts.clone(), env, &mut db)?;
                results.push(TraceResult::Success { result });
            }

            Ok(results)
        })
    }

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

        self.eth_api.with_state_at(at, |state| {
            let tx = transaction.into_recovered();
            let tx = tx_env_with_recovered(&tx);
            let env = Env { cfg, block, tx };
            let mut db = SubState::new(State::new(state));
            trace_transaction(opts, env, &mut db)
        })
    }

    /// The debug_traceCall method lets you run an `eth_call` within the context of the given block
    /// execution using the final state of parent block as the base.
    pub async fn debug_trace_call(
        &self,
        call: CallRequest,
        block_id: Option<BlockId>,
        opts: GethDebugTracingCallOptions,
    ) -> EthResult<GethTraceFrame> {
        let at = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        // TODO(mattsse) apply block overrides
        let GethDebugTracingCallOptions { tracing_options, state_overrides, block_overrides: _ } =
            opts;
        let GethDebugTracingOptions { config, .. } = tracing_options;
        // TODO(mattsse) support non default tracers

        // default structlog tracer
        let inspector_config = TracingInspectorConfig::from_geth_config(&config);

        let mut inspector = TracingInspector::new(inspector_config);

        let (res, _) =
            self.eth_api.inspect_call_at(call, at, state_overrides, &mut inspector).await?;
        let gas_used = res.result.gas_used();

        let frame = inspector.into_geth_builder().geth_traces(U256::from(gas_used), config);

        Ok(frame.into())
    }
}

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
        rlp_block: Bytes,
        opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Ok(DebugApi::debug_trace_raw_block(self, rlp_block, opts).await?)
    }

    /// Handler for `debug_traceBlockByHash`
    async fn debug_trace_block_by_hash(
        &self,
        block: H256,
        opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Ok(DebugApi::debug_trace_block(self, block.into(), opts).await?)
    }

    /// Handler for `debug_traceBlockByNumber`
    async fn debug_trace_block_by_number(
        &self,
        block: BlockNumberOrTag,
        opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Ok(DebugApi::debug_trace_block(self, block.into(), opts).await?)
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
        request: CallRequest,
        block_number: Option<BlockId>,
        opts: GethDebugTracingCallOptions,
    ) -> RpcResult<GethTraceFrame> {
        Ok(DebugApi::debug_trace_call(self, request, block_number, opts).await?)
    }
}

impl<Client, Eth> std::fmt::Debug for DebugApi<Client, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}

/// Executes the configured transaction in the environment on the given database.
///
/// Note: this does not apply any state overrides if they're configured in the `opts`.
fn trace_transaction(
    opts: GethDebugTracingOptions,
    env: Env,
    db: &mut SubState<StateProviderBox<'_>>,
) -> EthResult<GethTraceFrame> {
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
                    let mut inspector = FourByteInspector::default();
                    let _ = inspect(db, env, &mut inspector)?;
                    return Ok(FourByteFrame::from(inspector).into())
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
    let inspector_config = TracingInspectorConfig::from_geth_config(&config);

    let mut inspector = TracingInspector::new(inspector_config);

    let (res, _) = inspect(db, env, &mut inspector)?;
    let gas_used = res.result.gas_used();

    let frame = inspector.into_geth_builder().geth_traces(U256::from(gas_used), config);

    Ok(frame.into())
}
