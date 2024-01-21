use crate::{
    eth::{
        error::{EthApiError, EthResult},
        revm_utils::{
            inspect, inspect_and_return_db, prepare_call_env, replay_transactions_until, transact,
            EvmOverrides,
        },
        EthTransactions, TransactionSource,
    },
    result::{internal_rpc_err, ToRpcResult},
    BlockingTaskGuard, EthApiSpec,
};
use alloy_rlp::{Decodable, Encodable};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{
    revm::env::tx_env_with_recovered,
    revm_primitives::{db::DatabaseCommit, BlockEnv, CfgEnv},
    Address, Block, BlockId, BlockNumberOrTag, Bytes, TransactionSignedEcRecovered, B256,
};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, HeaderProvider, StateProviderBox, TransactionVariant,
};
use reth_revm::{
    database::{StateProviderDatabase, SubState},
    tracing::{js::JsInspector, FourByteInspector, TracingInspector, TracingInspectorConfig},
};
use reth_rpc_api::DebugApiServer;
use reth_rpc_types::{
    trace::geth::{
        BlockTraceResult, FourByteFrame, GethDebugBuiltInTracerType, GethDebugTracerType,
        GethDebugTracingCallOptions, GethDebugTracingOptions, GethTrace, NoopFrame, TraceResult,
    },
    BlockError, Bundle, CallRequest, RichBlock, StateContext,
};
use revm::{db::CacheDB, primitives::Env};
use std::sync::Arc;
use tokio::sync::{AcquireError, OwnedSemaphorePermit};

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
pub struct DebugApi<Provider, Eth> {
    inner: Arc<DebugApiInner<Provider, Eth>>,
}

// === impl DebugApi ===

impl<Provider, Eth> DebugApi<Provider, Eth> {
    /// Create a new instance of the [DebugApi]
    pub fn new(provider: Provider, eth: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        let inner = Arc::new(DebugApiInner { provider, eth_api: eth, blocking_task_guard });
        Self { inner }
    }
}

// === impl DebugApi ===

impl<Provider, Eth> DebugApi<Provider, Eth>
where
    Provider: BlockReaderIdExt + HeaderProvider + ChainSpecProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(&self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.inner.blocking_task_guard.clone().acquire_owned().await
    }

    /// Trace the entire block asynchronously
    async fn trace_block_with(
        &self,
        at: BlockId,
        transactions: Vec<TransactionSignedEcRecovered>,
        cfg: CfgEnv,
        block_env: BlockEnv,
        opts: GethDebugTracingOptions,
    ) -> EthResult<Vec<TraceResult>> {
        // replay all transactions of the block
        let this = self.clone();
        self.inner
            .eth_api
            .spawn_with_state_at_block(at, move |state| {
                let mut results = Vec::with_capacity(transactions.len());
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                let mut transactions = transactions.into_iter().peekable();
                while let Some(tx) = transactions.next() {
                    let tx_hash = tx.hash;
                    let tx = tx_env_with_recovered(&tx);
                    let env = Env { cfg: cfg.clone(), block: block_env.clone(), tx };
                    let (result, state_changes) =
                        this.trace_transaction(opts.clone(), env, &mut db).map_err(|err| {
                            results.push(TraceResult::Error {
                                error: err.to_string(),
                                tx_hash: Some(tx_hash),
                            });
                            err
                        })?;

                    results.push(TraceResult::Success { result, tx_hash: Some(tx_hash) });
                    if transactions.peek().is_some() {
                        // need to apply the state changes of this transaction before executing the
                        // next transaction
                        db.commit(state_changes)
                    }
                }

                Ok(results)
            })
            .await
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

        let (cfg, block_env) = self.inner.eth_api.evm_env_for_raw_block(&block.header).await?;
        // we trace on top the block's parent block
        let parent = block.parent_hash;

        // Depending on EIP-2 we need to recover the transactions differently
        let transactions =
            if self.inner.provider.chain_spec().is_homestead_active_at_block(block.number) {
                block
                    .body
                    .into_iter()
                    .map(|tx| {
                        tx.into_ecrecovered()
                            .ok_or_else(|| EthApiError::InvalidTransactionSignature)
                    })
                    .collect::<EthResult<Vec<_>>>()?
            } else {
                block
                    .body
                    .into_iter()
                    .map(|tx| {
                        tx.into_ecrecovered_unchecked()
                            .ok_or_else(|| EthApiError::InvalidTransactionSignature)
                    })
                    .collect::<EthResult<Vec<_>>>()?
            };

        self.trace_block_with(parent.into(), transactions, cfg, block_env, opts).await
    }

    /// Replays a block and returns the trace of each transaction.
    pub async fn debug_trace_block(
        &self,
        block_id: BlockId,
        opts: GethDebugTracingOptions,
    ) -> EthResult<Vec<TraceResult>> {
        let block_hash = self
            .inner
            .provider
            .block_hash_for_id(block_id)?
            .ok_or_else(|| EthApiError::UnknownBlockNumber)?;

        let ((cfg, block_env, _), block) = futures::try_join!(
            self.inner.eth_api.evm_env_at(block_hash.into()),
            self.inner.eth_api.block_by_id_with_senders(block_id),
        )?;

        let block = block.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        // we need to get the state of the parent block because we're replaying this block on top of
        // its parent block's state
        let state_at = block.parent_hash;

        self.trace_block_with(
            state_at.into(),
            block.into_transactions_ecrecovered().collect(),
            cfg,
            block_env,
            opts,
        )
        .await
    }

    /// Trace the transaction according to the provided options.
    ///
    /// Ref: <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>
    pub async fn debug_trace_transaction(
        &self,
        tx_hash: B256,
        opts: GethDebugTracingOptions,
    ) -> EthResult<GethTrace> {
        let (transaction, block) = match self.inner.eth_api.transaction_and_block(tx_hash).await? {
            None => return Err(EthApiError::TransactionNotFound),
            Some(res) => res,
        };
        let (cfg, block_env, _) = self.inner.eth_api.evm_env_at(block.hash.into()).await?;

        // we need to get the state of the parent block because we're essentially replaying the
        // block the transaction is included in
        let state_at: BlockId = block.parent_hash.into();
        let block_txs = block.body;

        let this = self.clone();
        self.inner
            .eth_api
            .spawn_with_state_at_block(state_at, move |state| {
                // configure env for the target transaction
                let tx = transaction.into_recovered();

                let mut db = CacheDB::new(StateProviderDatabase::new(state));
                // replay all transactions prior to the targeted transaction
                replay_transactions_until(
                    &mut db,
                    cfg.clone(),
                    block_env.clone(),
                    block_txs,
                    tx.hash,
                )?;

                let env = Env { cfg, block: block_env, tx: tx_env_with_recovered(&tx) };
                this.trace_transaction(opts, env, &mut db).map(|(trace, _)| trace)
            })
            .await
    }

    /// The debug_traceCall method lets you run an `eth_call` within the context of the given block
    /// execution using the final state of parent block as the base.
    pub async fn debug_trace_call(
        &self,
        call: CallRequest,
        block_id: Option<BlockId>,
        opts: GethDebugTracingCallOptions,
    ) -> EthResult<GethTrace> {
        let at = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let GethDebugTracingCallOptions { tracing_options, state_overrides, block_overrides } =
            opts;
        let overrides = EvmOverrides::new(state_overrides, block_overrides.map(Box::new));
        let GethDebugTracingOptions { config, tracer, tracer_config, .. } = tracing_options;

        if let Some(tracer) = tracer {
            return match tracer {
                GethDebugTracerType::BuiltInTracer(tracer) => match tracer {
                    GethDebugBuiltInTracerType::FourByteTracer => {
                        let mut inspector = FourByteInspector::default();
                        let inspector = self
                            .inner
                            .eth_api
                            .spawn_with_call_at(call, at, overrides, move |db, env| {
                                inspect(db, env, &mut inspector)?;
                                Ok(inspector)
                            })
                            .await?;
                        return Ok(FourByteFrame::from(inspector).into())
                    }
                    GethDebugBuiltInTracerType::CallTracer => {
                        let call_config = tracer_config
                            .into_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_config(&config)
                                .set_record_logs(call_config.with_log.unwrap_or_default()),
                        );

                        let frame = self
                            .inner
                            .eth_api
                            .spawn_with_call_at(call, at, overrides, move |db, env| {
                                let (res, _) = inspect(db, env, &mut inspector)?;
                                let frame = inspector
                                    .into_geth_builder()
                                    .geth_call_traces(call_config, res.result.gas_used());
                                Ok(frame.into())
                            })
                            .await?;
                        return Ok(frame)
                    }
                    GethDebugBuiltInTracerType::PreStateTracer => {
                        let prestate_config = tracer_config
                            .into_pre_state_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;
                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_config(&config)
                                // if in default mode, we need to return all touched storages, for
                                // which we need to record steps and statediff
                                .set_steps_and_state_diffs(prestate_config.is_default_mode()),
                        );

                        let frame =
                            self.inner
                                .eth_api
                                .spawn_with_call_at(call, at, overrides, move |db, env| {
                                    let (res, _, db) =
                                        inspect_and_return_db(db, env, &mut inspector)?;
                                    let frame = inspector
                                        .into_geth_builder()
                                        .geth_prestate_traces(&res, prestate_config, &db)?;
                                    Ok(frame)
                                })
                                .await?;
                        return Ok(frame.into())
                    }
                    GethDebugBuiltInTracerType::NoopTracer => Ok(NoopFrame::default().into()),
                },
                GethDebugTracerType::JsTracer(code) => {
                    let config = tracer_config.into_json();

                    let (_, _, at) = self.inner.eth_api.evm_env_at(at).await?;

                    let res = self
                        .inner
                        .eth_api
                        .spawn_with_call_at(call, at, overrides, move |db, env| {
                            let mut inspector = JsInspector::new(code, config)?;
                            let (res, _, db) =
                                inspect_and_return_db(db, env.clone(), &mut inspector)?;
                            Ok(inspector.json_result(res, &env, &db)?)
                        })
                        .await?;

                    Ok(GethTrace::JS(res))
                }
            }
        }

        // default structlog tracer
        let inspector_config = TracingInspectorConfig::from_geth_config(&config);

        let mut inspector = TracingInspector::new(inspector_config);

        let (res, inspector) = self
            .inner
            .eth_api
            .spawn_with_call_at(call, at, overrides, move |db, env| {
                let (res, _) = inspect(db, env, &mut inspector)?;
                Ok((res, inspector))
            })
            .await?;
        let gas_used = res.result.gas_used();
        let return_value = res.result.into_output().unwrap_or_default();
        let frame = inspector.into_geth_builder().geth_traces(gas_used, return_value, config);

        Ok(frame.into())
    }

    /// The debug_traceCallMany method lets you run an `eth_callMany` within the context of the
    /// given block execution using the first n transactions in the given block as base
    pub async fn debug_trace_call_many(
        &self,
        bundles: Vec<Bundle>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> EthResult<Vec<Vec<GethTrace>>> {
        if bundles.is_empty() {
            return Err(EthApiError::InvalidParams(String::from("bundles are empty.")))
        }

        let StateContext { transaction_index, block_number } = state_context.unwrap_or_default();
        let transaction_index = transaction_index.unwrap_or_default();

        let target_block = block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let ((cfg, block_env, _), block) = futures::try_join!(
            self.inner.eth_api.evm_env_at(target_block),
            self.inner.eth_api.block_by_id_with_senders(target_block),
        )?;

        let opts = opts.unwrap_or_default();
        let block = block.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        let GethDebugTracingCallOptions { tracing_options, mut state_overrides, .. } = opts;
        let gas_limit = self.inner.eth_api.call_gas_limit();

        // we're essentially replaying the transactions in the block here, hence we need the state
        // that points to the beginning of the block, which is the state at the parent block
        let mut at = block.parent_hash;
        let mut replay_block_txs = true;

        // but if all transactions are to be replayed, we can use the state at the block itself
        let num_txs = transaction_index.index().unwrap_or(block.body.len());
        if num_txs == block.body.len() {
            at = block.hash;
            replay_block_txs = false;
        }

        let this = self.clone();
        self.inner
            .eth_api
            .spawn_with_state_at_block(at.into(), move |state| {
                // the outer vec for the bundles
                let mut all_bundles = Vec::with_capacity(bundles.len());
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                if replay_block_txs {
                    // only need to replay the transactions in the block if not all transactions are
                    // to be replayed
                    let transactions = block.into_transactions_ecrecovered().take(num_txs);

                    // Execute all transactions until index
                    for tx in transactions {
                        let tx = tx_env_with_recovered(&tx);
                        let env = Env { cfg: cfg.clone(), block: block_env.clone(), tx };
                        let (res, _) = transact(&mut db, env)?;
                        db.commit(res.state);
                    }
                }

                // Trace all bundles
                let mut bundles = bundles.into_iter().peekable();
                while let Some(bundle) = bundles.next() {
                    let mut results = Vec::with_capacity(bundle.transactions.len());
                    let Bundle { transactions, block_override } = bundle;

                    let block_overrides = block_override.map(Box::new);

                    let mut transactions = transactions.into_iter().peekable();
                    while let Some(tx) = transactions.next() {
                        // apply state overrides only once, before the first transaction
                        let state_overrides = state_overrides.take();
                        let overrides = EvmOverrides::new(state_overrides, block_overrides.clone());

                        let env = prepare_call_env(
                            cfg.clone(),
                            block_env.clone(),
                            tx,
                            gas_limit,
                            &mut db,
                            overrides,
                        )?;

                        let (trace, state) =
                            this.trace_transaction(tracing_options.clone(), env, &mut db)?;

                        // If there is more transactions, commit the database
                        // If there is no transactions, but more bundles, commit to the database too
                        if transactions.peek().is_some() || bundles.peek().is_some() {
                            db.commit(state);
                        }
                        results.push(trace);
                    }

                    all_bundles.push(results);
                }
                Ok(all_bundles)
            })
            .await
    }

    /// Executes the configured transaction with the environment on the given database.
    ///
    /// Returns the trace frame and the state that got updated after executing the transaction.
    ///
    /// Note: this does not apply any state overrides if they're configured in the `opts`.
    ///
    /// Caution: this is blocking and should be performed on a blocking task.
    fn trace_transaction(
        &self,
        opts: GethDebugTracingOptions,
        env: Env,
        db: &mut SubState<StateProviderBox>,
    ) -> EthResult<(GethTrace, revm_primitives::State)> {
        let GethDebugTracingOptions { config, tracer, tracer_config, .. } = opts;

        if let Some(tracer) = tracer {
            return match tracer {
                GethDebugTracerType::BuiltInTracer(tracer) => match tracer {
                    GethDebugBuiltInTracerType::FourByteTracer => {
                        let mut inspector = FourByteInspector::default();
                        let (res, _) = inspect(db, env, &mut inspector)?;
                        return Ok((FourByteFrame::from(inspector).into(), res.state))
                    }
                    GethDebugBuiltInTracerType::CallTracer => {
                        let call_config = tracer_config
                            .into_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_config(&config)
                                .set_record_logs(call_config.with_log.unwrap_or_default()),
                        );

                        let (res, _) = inspect(db, env, &mut inspector)?;

                        let frame = inspector
                            .into_geth_builder()
                            .geth_call_traces(call_config, res.result.gas_used());

                        return Ok((frame.into(), res.state))
                    }
                    GethDebugBuiltInTracerType::PreStateTracer => {
                        let prestate_config = tracer_config
                            .into_pre_state_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_config(&config)
                                // if in default mode, we need to return all touched storages, for
                                // which we need to record steps and statediff
                                .set_steps_and_state_diffs(prestate_config.is_default_mode()),
                        );
                        let (res, _) = inspect(&mut *db, env, &mut inspector)?;

                        let frame = inspector.into_geth_builder().geth_prestate_traces(
                            &res,
                            prestate_config,
                            &*db,
                        )?;

                        return Ok((frame.into(), res.state))
                    }
                    GethDebugBuiltInTracerType::NoopTracer => {
                        Ok((NoopFrame::default().into(), Default::default()))
                    }
                },
                GethDebugTracerType::JsTracer(code) => {
                    let config = tracer_config.into_json();

                    let mut inspector = JsInspector::new(code, config)?;
                    let (res, env, db) = inspect_and_return_db(db, env, &mut inspector)?;

                    let state = res.state.clone();
                    let result = inspector.json_result(res, &env, db)?;
                    Ok((GethTrace::JS(result), state))
                }
            }
        }

        // default structlog tracer
        let inspector_config = TracingInspectorConfig::from_geth_config(&config);

        let mut inspector = TracingInspector::new(inspector_config);

        let (res, _) = inspect(db, env, &mut inspector)?;
        let gas_used = res.result.gas_used();
        let return_value = res.result.into_output().unwrap_or_default();
        let frame = inspector.into_geth_builder().geth_traces(gas_used, return_value, config);

        Ok((frame.into(), res.state))
    }
}

#[async_trait]
impl<Provider, Eth> DebugApiServer for DebugApi<Provider, Eth>
where
    Provider: BlockReaderIdExt + HeaderProvider + ChainSpecProvider + 'static,
    Eth: EthApiSpec + 'static,
{
    /// Handler for `debug_getRawHeader`
    async fn raw_header(&self, block_id: BlockId) -> RpcResult<Bytes> {
        let header = match block_id {
            BlockId::Hash(hash) => self.inner.provider.header(&hash.into()).to_rpc_result()?,
            BlockId::Number(number_or_tag) => {
                let number = self
                    .inner
                    .provider
                    .convert_block_number(number_or_tag)
                    .to_rpc_result()?
                    .ok_or_else(|| internal_rpc_err("Pending block not supported".to_string()))?;
                self.inner.provider.header_by_number(number).to_rpc_result()?
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
        let block = self.inner.provider.block_by_id(block_id).to_rpc_result()?;

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
    async fn raw_transaction(&self, hash: B256) -> RpcResult<Bytes> {
        let tx = self.inner.eth_api.transaction_by_hash(hash).await?;
        Ok(tx
            .map(TransactionSource::into_recovered)
            .map(|tx| tx.envelope_encoded())
            .unwrap_or_default())
    }

    /// Handler for `debug_getRawTransactions`
    /// Returns the bytes of the transaction for the given hash.
    async fn raw_transactions(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>> {
        let block = self
            .inner
            .provider
            .block_with_senders_by_id(block_id, TransactionVariant::NoHash)
            .to_rpc_result()?
            .unwrap_or_default();
        Ok(block.into_transactions_ecrecovered().map(|tx| tx.envelope_encoded()).collect())
    }

    /// Handler for `debug_getRawReceipts`
    async fn raw_receipts(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>> {
        let receipts =
            self.inner.provider.receipts_by_block_id(block_id).to_rpc_result()?.unwrap_or_default();
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
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_raw_block(self, rlp_block, opts.unwrap_or_default()).await?)
    }

    /// Handler for `debug_traceBlockByHash`
    async fn debug_trace_block_by_hash(
        &self,
        block: B256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_block(self, block.into(), opts.unwrap_or_default()).await?)
    }

    /// Handler for `debug_traceBlockByNumber`
    async fn debug_trace_block_by_number(
        &self,
        block: BlockNumberOrTag,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_block(self, block.into(), opts.unwrap_or_default()).await?)
    }

    /// Handler for `debug_traceTransaction`
    async fn debug_trace_transaction(
        &self,
        tx_hash: B256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<GethTrace> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_transaction(self, tx_hash, opts.unwrap_or_default()).await?)
    }

    /// Handler for `debug_traceCall`
    async fn debug_trace_call(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<GethTrace> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_call(self, request, block_number, opts.unwrap_or_default())
            .await?)
    }

    async fn debug_trace_call_many(
        &self,
        bundles: Vec<Bundle>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<Vec<Vec<GethTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_call_many(self, bundles, state_context, opts).await?)
    }

    async fn debug_backtrace_at(&self, _location: &str) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_account_range(
        &self,
        _block_number: BlockNumberOrTag,
        _start: Bytes,
        _max_results: u64,
        _nocode: bool,
        _nostorage: bool,
        _incompletes: bool,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_block_profile(&self, _file: String, _seconds: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_chaindb_compact(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_chaindb_property(&self, _property: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_cpu_profile(&self, _file: String, _seconds: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_db_ancient(&self, _kind: String, _number: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_db_ancients(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_db_get(&self, _key: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_dump_block(&self, _number: BlockId) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_free_os_memory(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_freeze_client(&self, _node: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_gc_stats(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_get_accessible_state(
        &self,
        _from: BlockNumberOrTag,
        _to: BlockNumberOrTag,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_get_modified_accounts_by_hash(
        &self,
        _start_hash: B256,
        _end_hash: B256,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_get_modified_accounts_by_number(
        &self,
        _start_number: u64,
        _end_number: u64,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_go_trace(&self, _file: String, _seconds: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_intermediate_roots(
        &self,
        _block_hash: B256,
        _opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_mem_stats(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_mutex_profile(&self, _file: String, _nsec: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_preimage(&self, _hash: B256) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_print_block(&self, _number: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_seed_hash(&self, _number: u64) -> RpcResult<B256> {
        Ok(Default::default())
    }

    async fn debug_set_block_profile_rate(&self, _rate: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_set_gc_percent(&self, _v: i32) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_set_head(&self, _number: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_set_mutex_profile_fraction(&self, _rate: i32) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_set_trie_flush_interval(&self, _interval: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_stacks(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_standard_trace_bad_block_to_file(
        &self,
        _block: BlockNumberOrTag,
        _opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_standard_trace_block_to_file(
        &self,
        _block: BlockNumberOrTag,
        _opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_start_cpu_profile(&self, _file: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_start_go_trace(&self, _file: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_stop_cpu_profile(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_stop_go_trace(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_storage_range_at(
        &self,
        _block_hash: B256,
        _tx_idx: usize,
        _contract_address: Address,
        _key_start: B256,
        _max_result: u64,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_trace_bad_block(
        &self,
        _block_hash: B256,
        _opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_verbosity(&self, _level: usize) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_vmodule(&self, _pattern: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_write_block_profile(&self, _file: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_write_mem_profile(&self, _file: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_write_mutex_profile(&self, _file: String) -> RpcResult<()> {
        Ok(())
    }
}

impl<Provider, Eth> std::fmt::Debug for DebugApi<Provider, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}

impl<Provider, Eth> Clone for DebugApi<Provider, Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct DebugApiInner<Provider, Eth> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// The implementation of `eth` API
    eth_api: Eth,
    // restrict the number of concurrent calls to blocking calls
    blocking_task_guard: BlockingTaskGuard,
}
