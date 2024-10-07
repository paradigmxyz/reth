use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rlp::{Decodable, Encodable};
use alloy_rpc_types::{
    state::EvmOverrides, Block as RpcBlock, BlockError, Bundle, StateContext, TransactionInfo,
};
use alloy_rpc_types_debug::ExecutionWitness;
use alloy_rpc_types_eth::transaction::TransactionRequest;
use alloy_rpc_types_trace::geth::{
    call::FlatCallFrame, BlockTraceResult, FourByteFrame, GethDebugBuiltInTracerType,
    GethDebugTracerType, GethDebugTracingCallOptions, GethDebugTracingOptions, GethTrace,
    NoopFrame, TraceResult,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::EthereumHardforks;
use reth_evm::{
    execute::{BlockExecutorProvider, Executor},
    ConfigureEvmEnv,
};
use reth_primitives::{Block, BlockId, BlockNumberOrTag, TransactionSignedEcRecovered};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, HeaderProvider, StateProofProvider,
    StateProviderFactory, TransactionVariant,
};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_api::DebugApiServer;
use reth_rpc_eth_api::{
    helpers::{Call, EthApiSpec, EthTransactions, TraceExt},
    EthApiTypes, FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, StateCacheDb};
use reth_rpc_server_types::{result::internal_rpc_err, ToRpcResult};
use reth_tasks::pool::BlockingTaskGuard;
use reth_trie::{HashedPostState, HashedStorage};
use revm::{
    db::CacheDB,
    primitives::{db::DatabaseCommit, BlockEnv, CfgEnvWithHandlerCfg, Env, EnvWithHandlerCfg},
};
use revm_inspectors::tracing::{
    FourByteInspector, MuxInspector, TracingInspector, TracingInspectorConfig, TransactionContext,
};
use revm_primitives::{keccak256, HashMap};
use std::sync::Arc;
use tokio::sync::{AcquireError, OwnedSemaphorePermit};

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
pub struct DebugApi<Provider, Eth, BlockExecutor> {
    inner: Arc<DebugApiInner<Provider, Eth, BlockExecutor>>,
}

// === impl DebugApi ===

impl<Provider, Eth, BlockExecutor> DebugApi<Provider, Eth, BlockExecutor> {
    /// Create a new instance of the [`DebugApi`]
    pub fn new(
        provider: Provider,
        eth: Eth,
        blocking_task_guard: BlockingTaskGuard,
        block_executor: BlockExecutor,
    ) -> Self {
        let inner =
            Arc::new(DebugApiInner { provider, eth_api: eth, blocking_task_guard, block_executor });
        Self { inner }
    }

    /// Access the underlying `Eth` API.
    pub fn eth_api(&self) -> &Eth {
        &self.inner.eth_api
    }
}

// === impl DebugApi ===

impl<Provider, Eth, BlockExecutor> DebugApi<Provider, Eth, BlockExecutor>
where
    Provider: BlockReaderIdExt
        + HeaderProvider
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + EvmEnvProvider
        + 'static,
    Eth: EthApiTypes + TraceExt + 'static,
    BlockExecutor: BlockExecutorProvider,
{
    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(&self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.inner.blocking_task_guard.clone().acquire_owned().await
    }

    /// Trace the entire block asynchronously
    async fn trace_block(
        &self,
        at: BlockId,
        transactions: Vec<TransactionSignedEcRecovered>,
        cfg: CfgEnvWithHandlerCfg,
        block_env: BlockEnv,
        opts: GethDebugTracingOptions,
    ) -> Result<Vec<TraceResult>, Eth::Error> {
        if transactions.is_empty() {
            // nothing to trace
            return Ok(Vec::new())
        }

        // replay all transactions of the block
        let this = self.clone();
        self.eth_api()
            .spawn_with_state_at_block(at, move |state| {
                let block_hash = at.as_block_hash();
                let mut results = Vec::with_capacity(transactions.len());
                let mut db = CacheDB::new(StateProviderDatabase::new(state));
                let mut transactions = transactions.into_iter().enumerate().peekable();
                while let Some((index, tx)) = transactions.next() {
                    let tx_hash = tx.hash;

                    let env = EnvWithHandlerCfg {
                        env: Env::boxed(
                            cfg.cfg_env.clone(),
                            block_env.clone(),
                            Call::evm_config(this.eth_api()).tx_env(&tx),
                        ),
                        handler_cfg: cfg.handler_cfg,
                    };
                    let (result, state_changes) = this.trace_transaction(
                        opts.clone(),
                        env,
                        &mut db,
                        Some(TransactionContext {
                            block_hash,
                            tx_hash: Some(tx_hash),
                            tx_index: Some(index),
                        }),
                    )?;

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
    ) -> Result<Vec<TraceResult>, Eth::Error> {
        let block = Block::decode(&mut rlp_block.as_ref())
            .map_err(BlockError::RlpDecodeRawBlock)
            .map_err(Eth::Error::from_eth_err)?;

        let (cfg, block_env) = self.eth_api().evm_env_for_raw_block(&block.header).await?;
        // we trace on top the block's parent block
        let parent = block.parent_hash;

        // Depending on EIP-2 we need to recover the transactions differently
        let transactions =
            if self.inner.provider.chain_spec().is_homestead_active_at_block(block.number) {
                block
                    .body
                    .transactions
                    .into_iter()
                    .map(|tx| {
                        tx.into_ecrecovered()
                            .ok_or(EthApiError::InvalidTransactionSignature)
                            .map_err(Eth::Error::from_eth_err)
                    })
                    .collect::<Result<Vec<_>, Eth::Error>>()?
            } else {
                block
                    .body
                    .transactions
                    .into_iter()
                    .map(|tx| {
                        tx.into_ecrecovered_unchecked()
                            .ok_or(EthApiError::InvalidTransactionSignature)
                            .map_err(Eth::Error::from_eth_err)
                    })
                    .collect::<Result<Vec<_>, Eth::Error>>()?
            };

        self.trace_block(parent.into(), transactions, cfg, block_env, opts).await
    }

    /// Replays a block and returns the trace of each transaction.
    pub async fn debug_trace_block(
        &self,
        block_id: BlockId,
        opts: GethDebugTracingOptions,
    ) -> Result<Vec<TraceResult>, Eth::Error> {
        let block_hash = self
            .inner
            .provider
            .block_hash_for_id(block_id)
            .map_err(Eth::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(block_id))?;

        let ((cfg, block_env, _), block) = futures::try_join!(
            self.eth_api().evm_env_at(block_hash.into()),
            self.eth_api().block_with_senders(block_id),
        )?;

        let block = block.ok_or(EthApiError::HeaderNotFound(block_id))?;
        // we need to get the state of the parent block because we're replaying this block on top of
        // its parent block's state
        let state_at = block.parent_hash;

        self.trace_block(
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
    ) -> Result<GethTrace, Eth::Error> {
        let (transaction, block) = match self.eth_api().transaction_and_block(tx_hash).await? {
            None => return Err(EthApiError::TransactionNotFound.into()),
            Some(res) => res,
        };
        let (cfg, block_env, _) = self.eth_api().evm_env_at(block.hash().into()).await?;

        // we need to get the state of the parent block because we're essentially replaying the
        // block the transaction is included in
        let state_at: BlockId = block.parent_hash.into();
        let block_hash = block.hash();
        let block_txs = block.into_transactions_ecrecovered();

        let this = self.clone();
        self.eth_api()
            .spawn_with_state_at_block(state_at, move |state| {
                // configure env for the target transaction
                let tx = transaction.into_recovered();

                let mut db = CacheDB::new(StateProviderDatabase::new(state));
                // replay all transactions prior to the targeted transaction
                let index = this.eth_api().replay_transactions_until(
                    &mut db,
                    cfg.clone(),
                    block_env.clone(),
                    block_txs,
                    tx.hash,
                )?;

                let env = EnvWithHandlerCfg {
                    env: Env::boxed(
                        cfg.cfg_env.clone(),
                        block_env,
                        Call::evm_config(this.eth_api()).tx_env(&tx),
                    ),
                    handler_cfg: cfg.handler_cfg,
                };

                this.trace_transaction(
                    opts,
                    env,
                    &mut db,
                    Some(TransactionContext {
                        block_hash: Some(block_hash),
                        tx_index: Some(index),
                        tx_hash: Some(tx.hash),
                    }),
                )
                .map(|(trace, _)| trace)
            })
            .await
    }

    /// The `debug_traceCall` method lets you run an `eth_call` within the context of the given
    /// block execution using the final state of parent block as the base.
    ///
    /// Differences compare to `eth_call`:
    ///  - `debug_traceCall` executes with __enabled__ basefee check, `eth_call` does not: <https://github.com/paradigmxyz/reth/issues/6240>
    pub async fn debug_trace_call(
        &self,
        call: TransactionRequest,
        block_id: Option<BlockId>,
        opts: GethDebugTracingCallOptions,
    ) -> Result<GethTrace, Eth::Error> {
        let at = block_id.unwrap_or_default();
        let GethDebugTracingCallOptions { tracing_options, state_overrides, block_overrides } =
            opts;
        let overrides = EvmOverrides::new(state_overrides, block_overrides.map(Box::new));
        let GethDebugTracingOptions { config, tracer, tracer_config, .. } = tracing_options;

        let this = self.clone();
        if let Some(tracer) = tracer {
            return match tracer {
                GethDebugTracerType::BuiltInTracer(tracer) => match tracer {
                    GethDebugBuiltInTracerType::FourByteTracer => {
                        let mut inspector = FourByteInspector::default();
                        let inspector = self
                            .eth_api()
                            .spawn_with_call_at(call, at, overrides, move |db, env| {
                                this.eth_api().inspect(db, env, &mut inspector)?;
                                Ok(inspector)
                            })
                            .await?;
                        return Ok(FourByteFrame::from(&inspector).into())
                    }
                    GethDebugBuiltInTracerType::CallTracer => {
                        let call_config = tracer_config
                            .into_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_call_config(&call_config),
                        );

                        let frame = self
                            .eth_api()
                            .spawn_with_call_at(call, at, overrides, move |db, env| {
                                let (res, env) = this.eth_api().inspect(db, env, &mut inspector)?;
                                let frame = inspector
                                    .with_transaction_gas_limit(env.tx.gas_limit)
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
                            TracingInspectorConfig::from_geth_prestate_config(&prestate_config),
                        );

                        let frame = self
                            .eth_api()
                            .spawn_with_call_at(call, at, overrides, move |db, env| {
                                // wrapper is hack to get around 'higher-ranked lifetime error',
                                // see <https://github.com/rust-lang/rust/issues/100013>
                                let db = db.0;

                                let (res, env) =
                                    this.eth_api().inspect(&mut *db, env, &mut inspector)?;
                                let frame = inspector
                                    .with_transaction_gas_limit(env.tx.gas_limit)
                                    .into_geth_builder()
                                    .geth_prestate_traces(&res, &prestate_config, db)
                                    .map_err(Eth::Error::from_eth_err)?;
                                Ok(frame)
                            })
                            .await?;
                        return Ok(frame.into())
                    }
                    GethDebugBuiltInTracerType::NoopTracer => Ok(NoopFrame::default().into()),
                    GethDebugBuiltInTracerType::MuxTracer => {
                        let mux_config = tracer_config
                            .into_mux_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = MuxInspector::try_from_config(mux_config)
                            .map_err(Eth::Error::from_eth_err)?;

                        let frame = self
                            .inner
                            .eth_api
                            .spawn_with_call_at(call, at, overrides, move |db, env| {
                                // wrapper is hack to get around 'higher-ranked lifetime error', see
                                // <https://github.com/rust-lang/rust/issues/100013>
                                let db = db.0;

                                let (res, _) =
                                    this.eth_api().inspect(&mut *db, env, &mut inspector)?;
                                let frame = inspector
                                    .try_into_mux_frame(&res, db)
                                    .map_err(Eth::Error::from_eth_err)?;
                                Ok(frame.into())
                            })
                            .await?;
                        return Ok(frame)
                    }
                    GethDebugBuiltInTracerType::FlatCallTracer => {
                        let flat_call_config = tracer_config
                            .into_flat_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_flat_call_config(&flat_call_config),
                        );

                        let frame: FlatCallFrame = self
                            .inner
                            .eth_api
                            .spawn_with_call_at(call, at, overrides, move |db, env| {
                                let (_res, env) =
                                    this.eth_api().inspect(db, env, &mut inspector)?;
                                let tx_info = TransactionInfo::default();
                                let frame: FlatCallFrame = inspector
                                    .with_transaction_gas_limit(env.tx.gas_limit)
                                    .into_parity_builder()
                                    .into_localized_transaction_traces(tx_info);
                                Ok(frame)
                            })
                            .await?;

                        return Ok(frame.into());
                    }
                },
                #[cfg(not(feature = "js-tracer"))]
                GethDebugTracerType::JsTracer(_) => {
                    Err(EthApiError::Unsupported("JS Tracer is not enabled").into())
                }
                #[cfg(feature = "js-tracer")]
                GethDebugTracerType::JsTracer(code) => {
                    let config = tracer_config.into_json();

                    let (_, _, at) = self.eth_api().evm_env_at(at).await?;

                    let res = self
                        .eth_api()
                        .spawn_with_call_at(call, at, overrides, move |db, env| {
                            // wrapper is hack to get around 'higher-ranked lifetime error', see
                            // <https://github.com/rust-lang/rust/issues/100013>
                            let db = db.0;

                            let mut inspector =
                                revm_inspectors::tracing::js::JsInspector::new(code, config)
                                    .map_err(Eth::Error::from_eth_err)?;
                            let (res, _) =
                                this.eth_api().inspect(&mut *db, env.clone(), &mut inspector)?;
                            inspector.json_result(res, &env, db).map_err(Eth::Error::from_eth_err)
                        })
                        .await?;

                    Ok(GethTrace::JS(res))
                }
            }
        }

        // default structlog tracer
        let inspector_config = TracingInspectorConfig::from_geth_config(&config);

        let mut inspector = TracingInspector::new(inspector_config);

        let (res, tx_gas_limit, inspector) = self
            .eth_api()
            .spawn_with_call_at(call, at, overrides, move |db, env| {
                let (res, env) = this.eth_api().inspect(db, env, &mut inspector)?;
                Ok((res, env.tx.gas_limit, inspector))
            })
            .await?;
        let gas_used = res.result.gas_used();
        let return_value = res.result.into_output().unwrap_or_default();
        let frame = inspector
            .with_transaction_gas_limit(tx_gas_limit)
            .into_geth_builder()
            .geth_traces(gas_used, return_value, config);

        Ok(frame.into())
    }

    /// The `debug_traceCallMany` method lets you run an `eth_callMany` within the context of the
    /// given block execution using the first n transactions in the given block as base.
    /// Each following bundle increments block number by 1 and block timestamp by 12 seconds
    pub async fn debug_trace_call_many(
        &self,
        bundles: Vec<Bundle>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> Result<Vec<Vec<GethTrace>>, Eth::Error> {
        if bundles.is_empty() {
            return Err(EthApiError::InvalidParams(String::from("bundles are empty.")).into())
        }

        let StateContext { transaction_index, block_number } = state_context.unwrap_or_default();
        let transaction_index = transaction_index.unwrap_or_default();

        let target_block = block_number.unwrap_or_default();
        let ((cfg, mut block_env, _), block) = futures::try_join!(
            self.eth_api().evm_env_at(target_block),
            self.eth_api().block_with_senders(target_block),
        )?;

        let opts = opts.unwrap_or_default();
        let block = block.ok_or(EthApiError::HeaderNotFound(target_block))?;
        let GethDebugTracingCallOptions { tracing_options, mut state_overrides, .. } = opts;

        // we're essentially replaying the transactions in the block here, hence we need the state
        // that points to the beginning of the block, which is the state at the parent block
        let mut at = block.parent_hash;
        let mut replay_block_txs = true;

        // if a transaction index is provided, we need to replay the transactions until the index
        let num_txs = transaction_index.index().unwrap_or(block.body.transactions.len());
        // but if all transactions are to be replayed, we can use the state at the block itself
        // this works with the exception of the PENDING block, because its state might not exist if
        // built locally
        if !target_block.is_pending() && num_txs == block.body.transactions.len() {
            at = block.hash();
            replay_block_txs = false;
        }

        let this = self.clone();

        self.eth_api()
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
                        let env = EnvWithHandlerCfg {
                            env: Env::boxed(
                                cfg.cfg_env.clone(),
                                block_env.clone(),
                                Call::evm_config(this.eth_api()).tx_env(&tx),
                            ),
                            handler_cfg: cfg.handler_cfg,
                        };
                        let (res, _) = this.eth_api().transact(&mut db, env)?;
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

                        let env = this.eth_api().prepare_call_env(
                            cfg.clone(),
                            block_env.clone(),
                            tx,
                            &mut db,
                            overrides,
                        )?;

                        let (trace, state) =
                            this.trace_transaction(tracing_options.clone(), env, &mut db, None)?;

                        // If there is more transactions, commit the database
                        // If there is no transactions, but more bundles, commit to the database too
                        if transactions.peek().is_some() || bundles.peek().is_some() {
                            db.commit(state);
                        }
                        results.push(trace);
                    }
                    // Increment block_env number and timestamp for the next bundle
                    block_env.number += U256::from(1);
                    block_env.timestamp += U256::from(12);

                    all_bundles.push(results);
                }
                Ok(all_bundles)
            })
            .await
    }

    /// The `debug_executionWitness` method allows for re-execution of a block with the purpose of
    /// generating an execution witness. The witness comprises of a map of all hashed trie nodes
    /// to their preimages that were required during the execution of the block, including during
    /// state root recomputation.
    pub async fn debug_execution_witness(
        &self,
        block_id: BlockNumberOrTag,
    ) -> Result<ExecutionWitness, Eth::Error> {
        let this = self.clone();
        let block = this
            .eth_api()
            .block_with_senders(block_id.into())
            .await?
            .ok_or(EthApiError::HeaderNotFound(block_id.into()))?;

        self.eth_api()
            .spawn_with_state_at_block(block.parent_hash.into(), move |state_provider| {
                let db = StateProviderDatabase::new(&state_provider);
                let block_executor = this.inner.block_executor.executor(db);

                let mut hashed_state = HashedPostState::default();
                let mut keys = HashMap::default();
                let mut codes = HashMap::default();

                let _ = block_executor
                    .execute_with_state_witness(
                        (&block.clone().unseal(), block.difficulty).into(),
                        |statedb| {
                            codes = statedb
                                .cache
                                .contracts
                                .iter()
                                .map(|(hash, code)| (*hash, code.bytes()))
                                .collect();

                            for (address, account) in &statedb.cache.accounts {
                                let hashed_address = keccak256(address);
                                hashed_state.accounts.insert(
                                    hashed_address,
                                    account.account.as_ref().map(|a| a.info.clone().into()),
                                );

                                let storage =
                                    hashed_state.storages.entry(hashed_address).or_insert_with(
                                        || HashedStorage::new(account.status.was_destroyed()),
                                    );

                                if let Some(account) = &account.account {
                                    keys.insert(hashed_address, address.to_vec().into());

                                    for (slot, value) in &account.storage {
                                        let slot = B256::from(*slot);
                                        let hashed_slot = keccak256(slot);
                                        storage.storage.insert(hashed_slot, *value);

                                        keys.insert(hashed_slot, slot.into());
                                    }
                                }
                            }
                        },
                    )
                    .map_err(|err| EthApiError::Internal(err.into()))?;

                let state =
                    state_provider.witness(Default::default(), hashed_state).map_err(Into::into)?;
                Ok(ExecutionWitness {
                    state: HashMap::from_iter(state.into_iter()),
                    codes,
                    keys: Some(keys),
                })
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
        env: EnvWithHandlerCfg,
        db: &mut StateCacheDb<'_>,
        transaction_context: Option<TransactionContext>,
    ) -> Result<(GethTrace, revm_primitives::EvmState), Eth::Error> {
        let GethDebugTracingOptions { config, tracer, tracer_config, .. } = opts;

        if let Some(tracer) = tracer {
            return match tracer {
                GethDebugTracerType::BuiltInTracer(tracer) => match tracer {
                    GethDebugBuiltInTracerType::FourByteTracer => {
                        let mut inspector = FourByteInspector::default();
                        let (res, _) = self.eth_api().inspect(db, env, &mut inspector)?;
                        return Ok((FourByteFrame::from(&inspector).into(), res.state))
                    }
                    GethDebugBuiltInTracerType::CallTracer => {
                        let call_config = tracer_config
                            .into_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_call_config(&call_config),
                        );

                        let (res, env) = self.eth_api().inspect(db, env, &mut inspector)?;

                        let frame = inspector
                            .with_transaction_gas_limit(env.tx.gas_limit)
                            .into_geth_builder()
                            .geth_call_traces(call_config, res.result.gas_used());

                        return Ok((frame.into(), res.state))
                    }
                    GethDebugBuiltInTracerType::PreStateTracer => {
                        let prestate_config = tracer_config
                            .into_pre_state_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_geth_prestate_config(&prestate_config),
                        );
                        let (res, env) = self.eth_api().inspect(&mut *db, env, &mut inspector)?;

                        let frame = inspector
                            .with_transaction_gas_limit(env.tx.gas_limit)
                            .into_geth_builder()
                            .geth_prestate_traces(&res, &prestate_config, db)
                            .map_err(Eth::Error::from_eth_err)?;

                        return Ok((frame.into(), res.state))
                    }
                    GethDebugBuiltInTracerType::NoopTracer => {
                        Ok((NoopFrame::default().into(), Default::default()))
                    }
                    GethDebugBuiltInTracerType::MuxTracer => {
                        let mux_config = tracer_config
                            .into_mux_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = MuxInspector::try_from_config(mux_config)
                            .map_err(Eth::Error::from_eth_err)?;

                        let (res, _) = self.eth_api().inspect(&mut *db, env, &mut inspector)?;
                        let frame = inspector
                            .try_into_mux_frame(&res, db)
                            .map_err(Eth::Error::from_eth_err)?;
                        return Ok((frame.into(), res.state))
                    }
                    GethDebugBuiltInTracerType::FlatCallTracer => {
                        let flat_call_config = tracer_config
                            .into_flat_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        let mut inspector = TracingInspector::new(
                            TracingInspectorConfig::from_flat_call_config(&flat_call_config),
                        );

                        let (res, env) = self.eth_api().inspect(db, env, &mut inspector)?;

                        let tx_info = TransactionInfo {
                            hash: transaction_context.unwrap().tx_hash,
                            index: transaction_context.unwrap().tx_index.map(|index| index as u64),
                            block_hash: transaction_context.unwrap().block_hash,
                            block_number: Some(env.block.number.try_into().unwrap_or_default()),
                            base_fee: Some(env.block.basefee.try_into().unwrap_or_default()),
                        };
                        let frame: FlatCallFrame = inspector
                            .with_transaction_gas_limit(env.tx.gas_limit)
                            .into_parity_builder()
                            .into_localized_transaction_traces(tx_info);

                        return Ok((frame.into(), res.state));
                    }
                },
                #[cfg(not(feature = "js-tracer"))]
                GethDebugTracerType::JsTracer(_) => {
                    Err(EthApiError::Unsupported("JS Tracer is not enabled").into())
                }
                #[cfg(feature = "js-tracer")]
                GethDebugTracerType::JsTracer(code) => {
                    let config = tracer_config.into_json();
                    let mut inspector =
                        revm_inspectors::tracing::js::JsInspector::with_transaction_context(
                            code,
                            config,
                            transaction_context.unwrap_or_default(),
                        )
                        .map_err(Eth::Error::from_eth_err)?;
                    let (res, env) = self.eth_api().inspect(&mut *db, env, &mut inspector)?;

                    let state = res.state.clone();
                    let result =
                        inspector.json_result(res, &env, db).map_err(Eth::Error::from_eth_err)?;
                    Ok((GethTrace::JS(result), state))
                }
            }
        }

        // default structlog tracer
        let inspector_config = TracingInspectorConfig::from_geth_config(&config);

        let mut inspector = TracingInspector::new(inspector_config);

        let (res, env) = self.eth_api().inspect(db, env, &mut inspector)?;
        let gas_used = res.result.gas_used();
        let return_value = res.result.into_output().unwrap_or_default();
        let frame = inspector
            .with_transaction_gas_limit(env.tx.gas_limit)
            .into_geth_builder()
            .geth_traces(gas_used, return_value, config);

        Ok((frame.into(), res.state))
    }
}

#[async_trait]
impl<Provider, Eth, BlockExecutor> DebugApiServer for DebugApi<Provider, Eth, BlockExecutor>
where
    Provider: BlockReaderIdExt
        + HeaderProvider
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + EvmEnvProvider
        + 'static,
    Eth: EthApiSpec + EthTransactions + TraceExt + 'static,
    BlockExecutor: BlockExecutorProvider,
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
        let block = self
            .inner
            .provider
            .block_by_id(block_id)
            .to_rpc_result()?
            .ok_or(EthApiError::HeaderNotFound(block_id))?;
        let mut res = Vec::new();
        block.encode(&mut res);
        Ok(res.into())
    }

    /// Handler for `debug_getRawTransaction`
    ///
    /// If this is a pooled EIP-4844 transaction, the blob sidecar is included.
    ///
    /// Returns the bytes of the transaction for the given hash.
    async fn raw_transaction(&self, hash: B256) -> RpcResult<Option<Bytes>> {
        self.eth_api().raw_transaction_by_hash(hash).await.map_err(Into::into)
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
        Ok(block.into_transactions_ecrecovered().map(|tx| tx.encoded_2718().into()).collect())
    }

    /// Handler for `debug_getRawReceipts`
    async fn raw_receipts(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>> {
        Ok(self
            .inner
            .provider
            .receipts_by_block_id(block_id)
            .to_rpc_result()?
            .unwrap_or_default()
            .into_iter()
            .map(|receipt| receipt.with_bloom().envelope_encoded())
            .collect())
    }

    /// Handler for `debug_getBadBlocks`
    async fn bad_blocks(&self) -> RpcResult<Vec<RpcBlock>> {
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
        Self::debug_trace_raw_block(self, rlp_block, opts.unwrap_or_default())
            .await
            .map_err(Into::into)
    }

    /// Handler for `debug_traceBlockByHash`
    async fn debug_trace_block_by_hash(
        &self,
        block: B256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_trace_block(self, block.into(), opts.unwrap_or_default())
            .await
            .map_err(Into::into)
    }

    /// Handler for `debug_traceBlockByNumber`
    async fn debug_trace_block_by_number(
        &self,
        block: BlockNumberOrTag,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_trace_block(self, block.into(), opts.unwrap_or_default())
            .await
            .map_err(Into::into)
    }

    /// Handler for `debug_traceTransaction`
    async fn debug_trace_transaction(
        &self,
        tx_hash: B256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<GethTrace> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_trace_transaction(self, tx_hash, opts.unwrap_or_default())
            .await
            .map_err(Into::into)
    }

    /// Handler for `debug_executionWitness`
    async fn debug_execution_witness(
        &self,
        block: BlockNumberOrTag,
    ) -> RpcResult<ExecutionWitness> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_execution_witness(self, block).await.map_err(Into::into)
    }

    /// Handler for `debug_traceCall`
    async fn debug_trace_call(
        &self,
        request: TransactionRequest,
        block_id: Option<BlockId>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<GethTrace> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_trace_call(self, request, block_id, opts.unwrap_or_default())
            .await
            .map_err(Into::into)
    }

    async fn debug_trace_call_many(
        &self,
        bundles: Vec<Bundle>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<Vec<Vec<GethTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_trace_call_many(self, bundles, state_context, opts).await.map_err(Into::into)
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

impl<Provider, Eth, BlockExecutor> std::fmt::Debug for DebugApi<Provider, Eth, BlockExecutor> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}

impl<Provider, Eth, BlockExecutor> Clone for DebugApi<Provider, Eth, BlockExecutor> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct DebugApiInner<Provider, Eth, BlockExecutor> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// The implementation of `eth` API
    eth_api: Eth,
    // restrict the number of concurrent calls to blocking calls
    blocking_task_guard: BlockingTaskGuard,
    /// block executor for debug & trace apis
    block_executor: BlockExecutor,
}
