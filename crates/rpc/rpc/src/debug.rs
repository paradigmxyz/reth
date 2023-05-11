use crate::{
    eth::{
        error::{EthApiError, EthResult},
        revm_utils::{inspect, replay_transactions_until},
        EthTransactions, TransactionSource,
    },
    result::{internal_rpc_err, ToRpcResult},
    EthApiSpec, TracingCallGuard,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{Block, BlockId, BlockNumberOrTag, Bytes, TransactionSigned, H256, U256};
use reth_provider::{BlockProviderIdExt, HeaderProvider, ReceiptProviderIdExt, StateProviderBox};
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
use revm_primitives::{db::DatabaseCommit, BlockEnv, CfgEnv};
use tokio::sync::{AcquireError, OwnedSemaphorePermit};

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
#[non_exhaustive]
pub struct DebugApi<Client, Eth> {
    /// The client that can interact with the chain.
    client: Client,
    /// The implementation of `eth` API
    eth_api: Eth,
    // restrict the number of concurrent calls to tracing calls
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
    Client: BlockProviderIdExt + HeaderProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(&self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.tracing_call_guard.clone().acquire_owned().await
    }

    /// Trace the entire block
    fn trace_block_with(
        &self,
        at: BlockId,
        transactions: Vec<TransactionSigned>,
        cfg: CfgEnv,
        block_env: BlockEnv,
        opts: GethDebugTracingOptions,
    ) -> EthResult<Vec<TraceResult>> {
        // replay all transactions of the block
        self.eth_api.with_state_at_block(at, move |state| {
            let mut results = Vec::with_capacity(transactions.len());
            let mut db = SubState::new(State::new(state));

            let mut transactions = transactions.into_iter().peekable();
            while let Some(tx) = transactions.next() {
                let tx = tx.into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
                let tx = tx_env_with_recovered(&tx);
                let env = Env { cfg: cfg.clone(), block: block_env.clone(), tx };
                let (result, state_changes) = trace_transaction(opts.clone(), env, &mut db)?;
                results.push(TraceResult::Success { result });

                if transactions.peek().is_some() {
                    // need to apply the state changes of this transaction before executing the next
                    // transaction
                    db.commit(state_changes)
                }
            }

            Ok(results)
        })
    }

    /// Replays the given block and returns the trace of each transaction.
    ///
    /// This expects a rlp encoded block
    ///
    /// Note, the parent of this block must be present, or it will fail.
    pub async fn debug_trace_raw_block(
        &self,
        rlp_block: Bytes,
        opts: GethDebugTracingOptions,
    ) -> EthResult<Vec<TraceResult>> {
        let block =
            Block::decode(&mut rlp_block.as_ref()).map_err(BlockError::RlpDecodeRawBlock)?;

        let (cfg, block_env) = self.eth_api.evm_env_for_raw_block(&block.header).await?;

        // we trace on top the block's parent block
        let parent = block.parent_hash;
        self.trace_block_with(parent.into(), block.body, cfg, block_env, opts)
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

        let ((cfg, block_env, _), block) = futures::try_join!(
            self.eth_api.evm_env_at(block_hash.into()),
            self.eth_api.block_by_id(block_id),
        )?;

        let block = block.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        // we need to get the state of the parent block because we're replaying this block on top of
        // its parent block's state
        let state_at = block.parent_hash;

        self.trace_block_with(state_at.into(), block.body, cfg, block_env, opts)
    }

    /// Trace the transaction according to the provided options.
    ///
    /// Ref: <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>
    pub async fn debug_trace_transaction(
        &self,
        tx_hash: H256,
        opts: GethDebugTracingOptions,
    ) -> EthResult<GethTraceFrame> {
        let (transaction, block) = match self.eth_api.transaction_and_block(tx_hash).await? {
            None => return Err(EthApiError::TransactionNotFound),
            Some(res) => res,
        };
        let (cfg, block_env, _) = self.eth_api.evm_env_at(block.hash.into()).await?;

        // we need to get the state of the parent block because we're essentially replaying the
        // block the transaction is included in
        let state_at = block.parent_hash;
        let block_txs = block.body;

        self.eth_api.with_state_at_block(state_at.into(), |state| {
            // configure env for the target transaction
            let tx = transaction.into_recovered();

            let mut db = SubState::new(State::new(state));
            // replay all transactions prior to the targeted transaction
            replay_transactions_until(&mut db, cfg.clone(), block_env.clone(), block_txs, tx.hash)?;

            let env = Env { cfg, block: block_env, tx: tx_env_with_recovered(&tx) };
            trace_transaction(opts, env, &mut db).map(|(trace, _)| trace)
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
        let GethDebugTracingOptions { config, tracer, tracer_config, .. } = tracing_options;

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
                        let (_res, _) = self
                            .eth_api
                            .inspect_call_at(call, at, state_overrides, &mut inspector)
                            .await?;
                        return Ok(FourByteFrame::from(inspector).into())
                    }
                    GethDebugBuiltInTracerType::CallTracer => {
                        // we validated the config above
                        let call_config =
                            tracer_config.and_then(|c| c.into_call_config()).unwrap_or_default();

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_config(&config),
                        );

                        let _ = self
                            .eth_api
                            .inspect_call_at(call, at, state_overrides, &mut inspector)
                            .await?;

                        let frame = inspector.into_geth_builder().geth_call_traces(call_config);

                        return Ok(frame.into())
                    }
                    GethDebugBuiltInTracerType::PreStateTracer => {
                        Err(EthApiError::Unsupported("pre state tracer currently unsupported."))
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
    Client: BlockProviderIdExt + HeaderProvider + 'static,
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
        let block = self.client.block_by_id(block_id).to_rpc_result()?;

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
        let receipts =
            self.client.receipts_by_block_id(block_id).to_rpc_result()?.unwrap_or_default();
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
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_raw_block(self, rlp_block, opts).await?)
    }

    /// Handler for `debug_traceBlockByHash`
    async fn debug_trace_block_by_hash(
        &self,
        block: H256,
        opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_block(self, block.into(), opts).await?)
    }

    /// Handler for `debug_traceBlockByNumber`
    async fn debug_trace_block_by_number(
        &self,
        block: BlockNumberOrTag,
        opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_block(self, block.into(), opts).await?)
    }

    /// Handler for `debug_traceTransaction`
    async fn debug_trace_transaction(
        &self,
        tx_hash: H256,
        opts: GethDebugTracingOptions,
    ) -> RpcResult<GethTraceFrame> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_transaction(self, tx_hash, opts).await?)
    }

    /// Handler for `debug_traceCall`
    async fn debug_trace_call(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
        opts: GethDebugTracingCallOptions,
    ) -> RpcResult<GethTraceFrame> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_call(self, request, block_number, opts).await?)
    }
}

impl<Client, Eth> std::fmt::Debug for DebugApi<Client, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}

/// Executes the configured transaction with the environment on the given database.
///
/// Returns the trace frame and the state that got updated after executing the transaction.
///
/// Note: this does not apply any state overrides if they're configured in the `opts`.
fn trace_transaction(
    opts: GethDebugTracingOptions,
    env: Env,
    db: &mut SubState<StateProviderBox<'_>>,
) -> EthResult<(GethTraceFrame, revm_primitives::State)> {
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
                    let (res, _) = inspect(db, env, &mut inspector)?;
                    return Ok((FourByteFrame::from(inspector).into(), res.state))
                }
                GethDebugBuiltInTracerType::CallTracer => {
                    // we validated the config above
                    let call_config =
                        tracer_config.and_then(|c| c.into_call_config()).unwrap_or_default();

                    let mut inspector =
                        TracingInspector::new(TracingInspectorConfig::from_geth_config(&config));

                    let (res, _) = inspect(db, env, &mut inspector)?;

                    let frame = inspector.into_geth_builder().geth_call_traces(call_config);

                    return Ok((frame.into(), res.state))
                }
                GethDebugBuiltInTracerType::PreStateTracer => {
                    todo!()
                }
                GethDebugBuiltInTracerType::NoopTracer => {
                    Ok((NoopFrame::default().into(), Default::default()))
                }
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

    Ok((frame.into(), res.state))
}
