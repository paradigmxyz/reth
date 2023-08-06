use crate::{
    eth::{
        error::{EthApiError, EthResult},
        revm_utils::{
            clone_into_empty_db, inspect, inspect_and_return_db, prepare_call_env,
            replay_transactions_until, result_output, transact, EvmOverrides,
        },
        EthTransactions, TransactionSource,
    },
    result::{internal_rpc_err, ToRpcResult},
    EthApiSpec, TracingCallGuard,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{Account, Block, BlockId, BlockNumberOrTag, Bytes, TransactionSigned, H256};
use reth_provider::{BlockReaderIdExt, HeaderProvider, StateProviderBox};
use reth_revm::{
    database::{State, SubState},
    env::tx_env_with_recovered,
    tracing::{
        js::{JsDbRequest, JsInspector},
        FourByteInspector, TracingInspector, TracingInspectorConfig,
    },
};
use reth_rlp::{Decodable, Encodable};
use reth_rpc_api::DebugApiServer;
use reth_rpc_types::{
    state::StateOverride,
    trace::geth::{
        BlockTraceResult, FourByteFrame, GethDebugBuiltInTracerType, GethDebugTracerType,
        GethDebugTracingCallOptions, GethDebugTracingOptions, GethTrace, NoopFrame, TraceResult,
    },
    BlockError, Bundle, CallRequest, RichBlock, StateContext,
};
use reth_tasks::TaskSpawner;
use revm::{
    db::{CacheDB, EmptyDB},
    primitives::Env,
};
use revm_primitives::{
    db::{DatabaseCommit, DatabaseRef},
    BlockEnv, CfgEnv,
};
use std::sync::Arc;
use tokio::sync::{mpsc, AcquireError, OwnedSemaphorePermit};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
pub struct DebugApi<Provider, Eth> {
    inner: Arc<DebugApiInner<Provider, Eth>>,
}

// === impl DebugApi ===

impl<Provider, Eth> DebugApi<Provider, Eth> {
    /// Create a new instance of the [DebugApi]
    pub fn new(
        provider: Provider,
        eth: Eth,
        task_spawner: Box<dyn TaskSpawner>,
        tracing_call_guard: TracingCallGuard,
    ) -> Self {
        let inner =
            Arc::new(DebugApiInner { provider, eth_api: eth, task_spawner, tracing_call_guard });
        Self { inner }
    }
}

// === impl DebugApi ===

impl<Provider, Eth> DebugApi<Provider, Eth>
where
    Provider: BlockReaderIdExt + HeaderProvider + 'static,
    Eth: EthTransactions + 'static,
{
    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(&self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.inner.tracing_call_guard.clone().acquire_owned().await
    }

    /// Trace the entire block asynchronously
    async fn trace_block_with(
        &self,
        at: BlockId,
        transactions: Vec<TransactionSigned>,
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
                let mut db = SubState::new(State::new(state));

                let mut transactions = transactions.into_iter().peekable();
                while let Some(tx) = transactions.next() {
                    let tx = tx.into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
                    let tx = tx_env_with_recovered(&tx);
                    let env = Env { cfg: cfg.clone(), block: block_env.clone(), tx };
                    let (result, state_changes) =
                        this.trace_transaction(opts.clone(), env, at, &mut db)?;
                    results.push(TraceResult::Success { result });

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
        self.trace_block_with(parent.into(), block.body, cfg, block_env, opts).await
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
            self.inner.eth_api.block_by_id(block_id),
        )?;

        let block = block.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        // we need to get the state of the parent block because we're replaying this block on top of
        // its parent block's state
        let state_at = block.parent_hash;

        self.trace_block_with(state_at.into(), block.body, cfg, block_env, opts).await
    }

    /// Trace the transaction according to the provided options.
    ///
    /// Ref: <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>
    pub async fn debug_trace_transaction(
        &self,
        tx_hash: H256,
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

                let mut db = SubState::new(State::new(state));
                // replay all transactions prior to the targeted transaction
                replay_transactions_until(
                    &mut db,
                    cfg.clone(),
                    block_env.clone(),
                    block_txs,
                    tx.hash,
                )?;

                let env = Env { cfg, block: block_env, tx: tx_env_with_recovered(&tx) };
                this.trace_transaction(opts, env, state_at, &mut db).map(|(trace, _)| trace)
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
                                inspect(db, env, &mut inspector)?;
                                let frame =
                                    inspector.into_geth_builder().geth_call_traces(call_config);
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
                            TracingInspectorConfig::from_geth_config(&config),
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

                    // for JS tracing we need to setup all async work before we can start tracing
                    // because JSTracer and all JS types are not Send
                    let (_, _, at) = self.inner.eth_api.evm_env_at(at).await?;
                    let state = self.inner.eth_api.state_at(at)?;
                    let db = SubState::new(State::new(state));
                    let has_state_overrides = overrides.has_state();

                    // If the caller provided state overrides we need to clone the DB so the js
                    // service has access these modifications
                    let mut maybe_override_db = None;
                    if has_state_overrides {
                        maybe_override_db = Some(clone_into_empty_db(&db));
                    }

                    let to_db_service = self.spawn_js_trace_service(at, maybe_override_db)?;

                    let res = self
                        .inner
                        .eth_api
                        .spawn_with_call_at(call, at, overrides, move |db, env| {
                            let mut inspector = JsInspector::new(code, config, to_db_service)?;
                            let (res, _) = inspect(db, env.clone(), &mut inspector)?;
                            Ok(inspector.json_result(res, &env)?)
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
        let return_value = result_output(&res.result).unwrap_or_default().into();
        let frame = inspector.into_geth_builder().geth_traces(gas_used, return_value, config);

        Ok(frame.into())
    }

    /// The debug_traceCallMany method lets you run an `eth_callMany` within the context of the
    /// given block execution using the first n transactions in the given block as base
    pub async fn debug_trace_call_many(
        &self,
        bundles: Vec<Bundle>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingOptions>,
        state_override: Option<StateOverride>,
    ) -> EthResult<Vec<GethTrace>> {
        if bundles.is_empty() {
            return Err(EthApiError::InvalidParams(String::from("bundles are empty.")))
        }

        let StateContext { transaction_index, block_number } = state_context.unwrap_or_default();
        let transaction_index = transaction_index.unwrap_or_default();

        let target_block = block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let ((cfg, block_env, _), block) = futures::try_join!(
            self.inner.eth_api.evm_env_at(target_block),
            self.inner.eth_api.block_by_id(target_block),
        )?;

        let block = block.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        let tracing_options = opts.unwrap_or_default();
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
                let mut results = Vec::with_capacity(bundles.len());
                let mut db = SubState::new(State::new(state));

                if replay_block_txs {
                    // only need to replay the transactions in the block if not all transactions are
                    // to be replayed
                    let transactions = block.body.into_iter().take(num_txs);

                    // Execute all transactions until index
                    for tx in transactions {
                        let tx = tx.into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
                        let tx = tx_env_with_recovered(&tx);
                        let env = Env { cfg: cfg.clone(), block: block_env.clone(), tx };
                        let (res, _) = transact(&mut db, env)?;
                        db.commit(res.state);
                    }
                }

                // Trace all bundles
                let mut bundles = bundles.into_iter().peekable();
                while let Some(bundle) = bundles.next() {
                    //let mut result = Vec::with_capacity(bundle.len());
                    let Bundle { transactions, block_override } = bundle;
                    let overrides =
                        EvmOverrides::new(state_override.clone(), block_override.map(Box::new));

                    let mut transactions = transactions.into_iter().peekable();
                    while let Some(tx) = transactions.next() {
                        let env = prepare_call_env(
                            cfg.clone(),
                            block_env.clone(),
                            tx,
                            gas_limit,
                            &mut db,
                            overrides.clone(),
                        )?;

                        let (trace, state) = this.trace_transaction(
                            tracing_options.clone(),
                            env,
                            target_block,
                            &mut db,
                        )?;

                        if bundles.peek().is_none() && transactions.peek().is_none() {
                            db.commit(state);
                        }
                        results.push(trace);
                    }
                }
                Ok(results)
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
        at: BlockId,
        db: &mut SubState<StateProviderBox<'_>>,
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

                        let frame = inspector.into_geth_builder().geth_call_traces(call_config);

                        return Ok((frame.into(), res.state))
                    }
                    GethDebugBuiltInTracerType::PreStateTracer => {
                        let prestate_config = tracer_config
                            .into_pre_state_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_config(&config),
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

                    // We need to clone the database because the JS tracer will need to access the
                    // current state via the spawned service
                    let js_db = clone_into_empty_db(db);
                    // we spawn the database service that will be used by the JS tracer
                    // transaction because the service needs access to the committed state changes
                    let to_db_service = self.spawn_js_trace_service(at, Some(js_db))?;

                    let mut inspector = JsInspector::new(code, config, to_db_service)?;
                    let (res, env) = inspect(db, env, &mut inspector)?;

                    let state = res.state.clone();
                    let result = inspector.json_result(res, &env)?;
                    Ok((GethTrace::JS(result), state))
                }
            }
        }

        // default structlog tracer
        let inspector_config = TracingInspectorConfig::from_geth_config(&config);

        let mut inspector = TracingInspector::new(inspector_config);

        let (res, _) = inspect(db, env, &mut inspector)?;
        let gas_used = res.result.gas_used();
        let return_value = result_output(&res.result).unwrap_or_default().into();
        let frame = inspector.into_geth_builder().geth_traces(gas_used, return_value, config);

        Ok((frame.into(), res.state))
    }

    /// Spawns [Self::js_trace_db_service_task] on a new task and returns a channel to send requests
    /// to it.
    ///
    /// Note: This blocks until the service is ready to receive requests.
    fn spawn_js_trace_service(
        &self,
        at: BlockId,
        db: Option<CacheDB<EmptyDB>>,
    ) -> EthResult<mpsc::Sender<JsDbRequest>> {
        let (to_db_service, rx) = mpsc::channel(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let this = self.clone();
        self.inner.task_spawner.spawn(Box::pin(async move {
            this.js_trace_db_service_task(at, rx, ready_tx, db).await
        }));
        // wait for initialization
        ready_rx.recv().map_err(|_| {
            EthApiError::InternalJsTracerError("js tracer initialization failed".to_string())
        })??;
        Ok(to_db_service)
    }

    /// A services that handles database requests issued from inside the JavaScript tracing engine.
    ///
    /// If this traces with modified state, this takes a `db` parameter that contains the modified
    /// in memory state. This is required because [StateProviderBox] can not be cloned or shared
    /// across threads.
    async fn js_trace_db_service_task(
        self,
        at: BlockId,
        rx: mpsc::Receiver<JsDbRequest>,
        on_ready: std::sync::mpsc::Sender<EthResult<()>>,
        db: Option<CacheDB<EmptyDB>>,
    ) {
        let state = match self.inner.eth_api.state_at(at) {
            Ok(state) => {
                let _ = on_ready.send(Ok(()));
                state
            }
            Err(err) => {
                let _ = on_ready.send(Err(err));
                return
            }
        };

        let db = if let Some(db) = db {
            let CacheDB { accounts, contracts, logs, block_hashes, .. } = db;
            CacheDB { accounts, contracts, logs, block_hashes, db: State::new(state) }
        } else {
            CacheDB::new(State::new(state))
        };

        let mut stream = ReceiverStream::new(rx);
        while let Some(req) = stream.next().await {
            match req {
                JsDbRequest::Basic { address, resp } => {
                    let acc = db
                        .basic(address)
                        .map(|maybe_acc| {
                            maybe_acc.map(|acc| Account {
                                nonce: acc.nonce,
                                balance: acc.balance,
                                bytecode_hash: Some(acc.code_hash),
                            })
                        })
                        .map_err(|err| err.to_string());
                    let _ = resp.send(acc);
                }
                JsDbRequest::Code { code_hash, resp } => {
                    let code = db
                        .code_by_hash(code_hash)
                        .map(|code| code.bytecode)
                        .map_err(|err| err.to_string());
                    let _ = resp.send(code);
                }
                JsDbRequest::StorageAt { address, index, resp } => {
                    let value = db.storage(address, index).map_err(|err| err.to_string());
                    let _ = resp.send(value);
                }
            }
        }
    }
}

#[async_trait]
impl<Provider, Eth> DebugApiServer for DebugApi<Provider, Eth>
where
    Provider: BlockReaderIdExt + HeaderProvider + 'static,
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
    async fn raw_transaction(&self, hash: H256) -> RpcResult<Bytes> {
        let tx = self.inner.eth_api.transaction_by_hash(hash).await?;

        let mut res = Vec::new();
        if let Some(tx) = tx.map(TransactionSource::into_recovered) {
            tx.encode(&mut res);
        }

        Ok(res.into())
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
        block: H256,
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
        tx_hash: H256,
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
        opts: Option<GethDebugTracingOptions>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<Vec<GethTrace>> {
        let _permit = self.acquire_trace_permit().await;
        Ok(DebugApi::debug_trace_call_many(self, bundles, state_context, opts, state_override)
            .await?)
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
    // restrict the number of concurrent calls to tracing calls
    tracing_call_guard: TracingCallGuard,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Box<dyn TaskSpawner>,
}
