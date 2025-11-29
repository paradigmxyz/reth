use alloy_consensus::{
    transaction::{SignerRecoverable, TxHashRef},
    BlockHeader,
};
use alloy_eips::{eip2718::Encodable2718, BlockId, BlockNumberOrTag};
use alloy_evm::{env::BlockEnvironment, Evm};
use alloy_genesis::ChainConfig;
use alloy_primitives::{hex::decode, uint, Address, Bytes, StorageKey, B256};
use alloy_rlp::{Decodable, Encodable};
use alloy_rpc_types_debug::{
    ExecutionWitness, StorageMap, StorageRangeResult as RpcStorageRangeResult, StorageResult,
};
use alloy_rpc_types_eth::{
    state::EvmOverrides, Block as RpcBlock, BlockError, Bundle, StateContext, TransactionInfo,
};
use alloy_rpc_types_trace::geth::{
    mux::MuxConfig, BlockTraceResult, CallConfig, FourByteFrame, GethDebugBuiltInTracerType,
    GethDebugTracerType, GethDebugTracingCallOptions, GethDebugTracingOptions,
    GethDefaultTracingOptions, GethTrace, NoopFrame, PreStateConfig, TraceResult,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_errors::RethError;
use reth_evm::{execute::Executor, ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::{Block as _, BlockBody, ReceiptWithBloom, RecoveredBlock};
use reth_revm::{db::State, witness::ExecutionWitnessRecord};
use reth_rpc_api::DebugApiServer;
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_api::{
    helpers::{EthTransactions, StorageDiffInspector, StorageDiffs, StorageRangeOverlay, TraceExt},
    EthApiTypes, FromEthApiError, FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::{EthApiError, StateCacheDb};
use reth_rpc_server_types::{result::internal_rpc_err, ToRpcResult};
use reth_storage_api::{
    BlockIdReader, BlockReaderIdExt, HeaderProvider, ProviderBlock, ReceiptProviderIdExt,
    StateProofProvider, StateProviderFactory, StateRootProvider, StorageRangeProvider,
    StorageRangeResult as ProviderStorageRangeResult, TransactionVariant,
};
use reth_tasks::pool::BlockingTaskGuard;
use reth_trie_common::{updates::TrieUpdates, HashedPostState};
use revm::{
    context::{
        result::{HaltReasonTr, ResultAndState},
        ContextTr,
    },
    inspector::{JournalExt, NoOpInspector},
    interpreter::{CallInputs, CallOutcome, CreateInputs, CreateOutcome, Interpreter},
    DatabaseCommit, DatabaseRef, Inspector,
};
use revm_inspectors::tracing::{
    FourByteInspector, MuxInspector, TracingInspector, TracingInspectorConfig, TransactionContext,
};
use revm_primitives::{Log, U256};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{AcquireError, OwnedSemaphorePermit};

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
pub struct DebugApi<Eth> {
    inner: Arc<DebugApiInner<Eth>>,
}

// === impl DebugApi ===

impl<Eth> DebugApi<Eth> {
    /// Create a new instance of the [`DebugApi`]
    pub fn new(eth_api: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        let inner = Arc::new(DebugApiInner { eth_api, blocking_task_guard });
        Self { inner }
    }

    /// Access the underlying `Eth` API.
    pub fn eth_api(&self) -> &Eth {
        &self.inner.eth_api
    }
}

impl<Eth: RpcNodeCore> DebugApi<Eth> {
    /// Access the underlying provider.
    pub fn provider(&self) -> &Eth::Provider {
        self.inner.eth_api.provider()
    }
}

// === impl DebugApi ===

impl<Eth> DebugApi<Eth>
where
    Eth: EthApiTypes + TraceExt + 'static,
{
    /// Acquires a permit to execute a tracing call.
    async fn acquire_trace_permit(&self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.inner.blocking_task_guard.clone().acquire_owned().await
    }

    /// Trace the entire block asynchronously
    async fn trace_block(
        &self,
        block: Arc<RecoveredBlock<ProviderBlock<Eth::Provider>>>,
        evm_env: EvmEnvFor<Eth::Evm>,
        opts: GethDebugTracingOptions,
    ) -> Result<Vec<TraceResult>, Eth::Error> {
        self.eth_api()
            .spawn_with_state_at_block(block.parent_hash(), move |eth_api, mut db| {
                let mut results = Vec::with_capacity(block.body().transactions().len());

                eth_api.apply_pre_execution_changes(&block, &mut db, &evm_env)?;

                let mut transactions = block.transactions_recovered().enumerate().peekable();
                let mut inspector = DebugInspector::new(opts)?;
                while let Some((index, tx)) = transactions.next() {
                    let tx_hash = *tx.tx_hash();
                    let tx_env = eth_api.evm_config().tx_env(tx);

                    let res = eth_api.inspect(
                        &mut db,
                        evm_env.clone(),
                        tx_env.clone(),
                        &mut inspector,
                    )?;
                    let result = inspector.get_result(
                        Some(TransactionContext {
                            block_hash: Some(block.hash()),
                            tx_hash: Some(tx_hash),
                            tx_index: Some(index),
                        }),
                        &tx_env,
                        &evm_env.block_env,
                        &res,
                        &mut db,
                    )?;

                    results.push(TraceResult::Success { result, tx_hash: Some(tx_hash) });
                    if transactions.peek().is_some() {
                        inspector.fuse()?;
                        // need to apply the state changes of this transaction before executing the
                        // next transaction
                        db.commit(res.state)
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
        let block: ProviderBlock<Eth::Provider> = Decodable::decode(&mut rlp_block.as_ref())
            .map_err(BlockError::RlpDecodeRawBlock)
            .map_err(Eth::Error::from_eth_err)?;

        let evm_env = self
            .eth_api()
            .evm_config()
            .evm_env(block.header())
            .map_err(RethError::other)
            .map_err(Eth::Error::from_eth_err)?;

        // Depending on EIP-2 we need to recover the transactions differently
        let senders =
            if self.provider().chain_spec().is_homestead_active_at_block(block.header().number()) {
                block
                    .body()
                    .transactions()
                    .iter()
                    .map(|tx| tx.recover_signer().map_err(Eth::Error::from_eth_err))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                block
                    .body()
                    .transactions()
                    .iter()
                    .map(|tx| tx.recover_signer_unchecked().map_err(Eth::Error::from_eth_err))
                    .collect::<Result<Vec<_>, _>>()?
            };

        self.trace_block(Arc::new(block.into_recovered_with_signers(senders)), evm_env, opts).await
    }

    /// Replays a block and returns the trace of each transaction.
    pub async fn debug_trace_block(
        &self,
        block_id: BlockId,
        opts: GethDebugTracingOptions,
    ) -> Result<Vec<TraceResult>, Eth::Error> {
        let block_hash = self
            .provider()
            .block_hash_for_id(block_id)
            .map_err(Eth::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(block_id))?;

        let ((evm_env, _), block) = futures::try_join!(
            self.eth_api().evm_env_at(block_hash.into()),
            self.eth_api().recovered_block(block_hash.into()),
        )?;

        let block = block.ok_or(EthApiError::HeaderNotFound(block_id))?;

        self.trace_block(block, evm_env, opts).await
    }

    /// Replays the transactions of `block_id` with [`StorageDiffInspector`] and returns the
    /// collected diffs.
    ///
    /// If `highest_tx_index` is provided, execution stops after the transaction at that index has
    /// been applied (inclusive).
    pub async fn run_with_storage_diff_inspector(
        &self,
        block_id: BlockId,
        highest_tx_index: Option<u64>,
    ) -> Result<StorageDiffs, Eth::Error> {
        let block_hash = self
            .provider()
            .block_hash_for_id(block_id)
            .map_err(Eth::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(block_id))
            .map_err(Eth::Error::from_eth_err)?;

        let max_index = match highest_tx_index {
            Some(idx) => Some(usize::try_from(idx).map_err(|_| {
                Eth::Error::from_eth_err(EthApiError::InvalidParams(
                    "tx index exceeds architecture limits".to_string(),
                ))
            })?),
            None => None,
        };

        let ((evm_env, _), block) = futures::try_join!(
            self.eth_api().evm_env_at(block_hash.into()),
            self.eth_api().recovered_block(block_hash.into()),
        )?;
        let block =
            block.ok_or(EthApiError::HeaderNotFound(block_id)).map_err(Eth::Error::from_eth_err)?;

        let parent_id: BlockId = block.parent_hash().into();
        let block = block.clone();
        let this = self.clone();

        self.eth_api()
            .spawn_with_state_at_block(parent_id, move |eth_api, mut db| {
                eth_api.apply_pre_execution_changes(&block, &mut db, &evm_env)?;

                let mut inspector = StorageDiffInspector::default();
                for (idx, tx) in block.transactions_recovered().enumerate() {
                    if let Some(limit) = max_index &&
                        idx > limit
                    {
                        break
                    }

                    let tx_env = this.eth_api().evm_config().tx_env(tx);
                    {
                        let mut evm = this.eth_api().evm_config().evm_with_env_and_inspector(
                            &mut db,
                            evm_env.clone(),
                            &mut inspector,
                        );
                        evm.transact_commit(tx_env).map_err(Eth::Error::from_evm_err)?;
                    }
                    inspector.finish_transaction();
                }

                Ok::<_, Eth::Error>(inspector.into_diffs())
            })
            .await
    }

    /// Computes the storage range for the given block, transaction index and account, taking into
    /// account the writes performed up to the specified transaction.
    pub async fn storage_range_at(
        &self,
        block_hash: B256,
        tx_idx: usize,
        contract_address: Address,
        key_start: B256,
        max_result: u64,
    ) -> Result<RpcStorageRangeResult, Eth::Error> {
        if max_result == 0 {
            return Err(
                EthApiError::InvalidParams("maxResult must be greater than zero".into()).into()
            )
        }

        let block_id: BlockId = block_hash.into();
        let block = self
            .eth_api()
            .recovered_block(block_id)
            .await?
            .ok_or(EthApiError::HeaderNotFound(block_id))?;
        let tx_count = block.body().transactions().len();
        if tx_idx >= tx_count {
            return Err(EthApiError::InvalidParams(format!(
                "transaction index {tx_idx} out of bounds for block with {tx_count} transactions"
            ))
            .into())
        }

        let diffs = self.run_with_storage_diff_inspector(block_id, Some(tx_idx as u64)).await?;

        let max_slots = usize::try_from(max_result).map_err(|_| {
            Eth::Error::from_eth_err(EthApiError::InvalidParams(
                "maxResult exceeds supported limits".to_string(),
            ))
        })?;
        let start_key = StorageKey::from(key_start);

        self.inner
            .eth_api
            .spawn_blocking_io(move |this| {
                let storage_provider = this
                    .provider()
                    .storage_range_by_block_hash(block_hash)
                    .map_err(Eth::Error::from_eth_err)?;
                let base = storage_provider.as_ref();

                let overlay_slots = StorageRangeOverlay::merged_slots_for_account(
                    base,
                    contract_address,
                    &diffs,
                    tx_idx,
                )
                .map_err(Eth::Error::from_eth_err)?;

                let range = if let Some(slots) = overlay_slots {
                    let mut overlay = StorageRangeOverlay::new(base);
                    overlay.add_account_overlay(contract_address, slots);
                    overlay.storage_range(contract_address, start_key, max_slots)
                } else {
                    base.storage_range(contract_address, start_key, max_slots)
                }
                .map_err(Eth::Error::from_eth_err)?;

                Ok(convert_storage_range(range))
            })
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
        let (evm_env, _) = self.eth_api().evm_env_at(block.hash().into()).await?;

        // we need to get the state of the parent block because we're essentially replaying the
        // block the transaction is included in
        let state_at: BlockId = block.parent_hash().into();
        let block_hash = block.hash();

        self.eth_api()
            .spawn_with_state_at_block(state_at, move |eth_api, mut db| {
                let block_txs = block.transactions_recovered();

                // configure env for the target transaction
                let tx = transaction.into_recovered();

                eth_api.apply_pre_execution_changes(&block, &mut db, &evm_env)?;

                // replay all transactions prior to the targeted transaction
                let index = eth_api.replay_transactions_until(
                    &mut db,
                    evm_env.clone(),
                    block_txs,
                    *tx.tx_hash(),
                )?;

                let tx_env = eth_api.evm_config().tx_env(&tx);

                let mut inspector = DebugInspector::new(opts)?;
                let res =
                    eth_api.inspect(&mut db, evm_env.clone(), tx_env.clone(), &mut inspector)?;
                let trace = inspector.get_result(
                    Some(TransactionContext {
                        block_hash: Some(block_hash),
                        tx_index: Some(index),
                        tx_hash: Some(*tx.tx_hash()),
                    }),
                    &tx_env,
                    &evm_env.block_env,
                    &res,
                    &mut db,
                )?;

                Ok(trace)
            })
            .await
    }

    /// The `debug_traceCall` method lets you run an `eth_call` within the context of the given
    /// block execution using the final state of parent block as the base.
    ///
    /// If `tx_index` is provided in opts, the call will be traced at the state after executing
    /// transactions up to the specified index within the block (0-indexed).
    /// If not provided, then uses the post-state (default behavior).
    ///
    /// Differences compare to `eth_call`:
    ///  - `debug_traceCall` executes with __enabled__ basefee check, `eth_call` does not: <https://github.com/paradigmxyz/reth/issues/6240>
    pub async fn debug_trace_call(
        &self,
        call: RpcTxReq<Eth::NetworkTypes>,
        block_id: Option<BlockId>,
        opts: GethDebugTracingCallOptions,
    ) -> Result<GethTrace, Eth::Error> {
        let at = block_id.unwrap_or_default();
        let GethDebugTracingCallOptions {
            tracing_options,
            state_overrides,
            block_overrides,
            tx_index,
        } = opts;
        let overrides = EvmOverrides::new(state_overrides, block_overrides.map(Box::new));

        // Check if we need to replay transactions for a specific tx_index
        if let Some(tx_idx) = tx_index {
            return self
                .debug_trace_call_at_tx_index(call, at, tx_idx as usize, tracing_options, overrides)
                .await;
        }

        let this = self.clone();
        self.eth_api()
            .spawn_with_call_at(call, at, overrides, move |db, evm_env, tx_env| {
                let mut inspector = DebugInspector::new(tracing_options)?;
                let res = this.eth_api().inspect(
                    &mut *db,
                    evm_env.clone(),
                    tx_env.clone(),
                    &mut inspector,
                )?;
                let trace = inspector.get_result(None, &tx_env, &evm_env.block_env, &res, db)?;
                Ok(trace)
            })
            .await
    }

    /// Helper method to execute `debug_trace_call` at a specific transaction index within a block.
    /// This replays transactions up to the specified index, then executes the trace call in that
    /// state.
    async fn debug_trace_call_at_tx_index(
        &self,
        call: RpcTxReq<Eth::NetworkTypes>,
        block_id: BlockId,
        tx_index: usize,
        tracing_options: GethDebugTracingOptions,
        overrides: EvmOverrides,
    ) -> Result<GethTrace, Eth::Error> {
        // Get the target block to check transaction count
        let block = self
            .eth_api()
            .recovered_block(block_id)
            .await?
            .ok_or(EthApiError::HeaderNotFound(block_id))?;

        if tx_index >= block.transaction_count() {
            // tx_index out of bounds
            return Err(EthApiError::InvalidParams(format!(
                "tx_index {} out of bounds for block with {} transactions",
                tx_index,
                block.transaction_count()
            ))
            .into())
        }

        let (evm_env, _) = self.eth_api().evm_env_at(block.hash().into()).await?;

        // execute after the parent block, replaying `tx_index` transactions
        let state_at = block.parent_hash();

        self.eth_api()
            .spawn_with_state_at_block(state_at, move |eth_api, mut db| {
                // 1. apply pre-execution changes
                eth_api.apply_pre_execution_changes(&block, &mut db, &evm_env)?;

                // 2. replay the required number of transactions
                for tx in block.transactions_recovered().take(tx_index) {
                    let tx_env = eth_api.evm_config().tx_env(tx);
                    let res = eth_api.transact(&mut db, evm_env.clone(), tx_env)?;
                    db.commit(res.state);
                }

                // 3. now execute the trace call on this state
                let (evm_env, tx_env) =
                    eth_api.prepare_call_env(evm_env, call, &mut db, overrides)?;

                let mut inspector = DebugInspector::new(tracing_options)?;
                let res =
                    eth_api.inspect(&mut db, evm_env.clone(), tx_env.clone(), &mut inspector)?;
                let trace =
                    inspector.get_result(None, &tx_env, &evm_env.block_env, &res, &mut db)?;

                Ok(trace)
            })
            .await
    }

    /// The `debug_traceCallMany` method lets you run an `eth_callMany` within the context of the
    /// given block execution using the first n transactions in the given block as base.
    /// Each following bundle increments block number by 1 and block timestamp by 12 seconds
    pub async fn debug_trace_call_many(
        &self,
        bundles: Vec<Bundle<RpcTxReq<Eth::NetworkTypes>>>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> Result<Vec<Vec<GethTrace>>, Eth::Error> {
        if bundles.is_empty() {
            return Err(EthApiError::InvalidParams(String::from("bundles are empty.")).into())
        }

        let StateContext { transaction_index, block_number } = state_context.unwrap_or_default();
        let transaction_index = transaction_index.unwrap_or_default();

        let target_block = block_number.unwrap_or_default();
        let ((mut evm_env, _), block) = futures::try_join!(
            self.eth_api().evm_env_at(target_block),
            self.eth_api().recovered_block(target_block),
        )?;

        let opts = opts.unwrap_or_default();
        let block = block.ok_or(EthApiError::HeaderNotFound(target_block))?;
        let GethDebugTracingCallOptions { tracing_options, mut state_overrides, .. } = opts;

        // we're essentially replaying the transactions in the block here, hence we need the state
        // that points to the beginning of the block, which is the state at the parent block
        let mut at = block.parent_hash();
        let mut replay_block_txs = true;

        // if a transaction index is provided, we need to replay the transactions until the index
        let num_txs =
            transaction_index.index().unwrap_or_else(|| block.body().transactions().len());
        // but if all transactions are to be replayed, we can use the state at the block itself
        // this works with the exception of the PENDING block, because its state might not exist if
        // built locally
        if !target_block.is_pending() && num_txs == block.body().transactions().len() {
            at = block.hash();
            replay_block_txs = false;
        }

        self.eth_api()
            .spawn_with_state_at_block(at, move |eth_api, mut db| {
                // the outer vec for the bundles
                let mut all_bundles = Vec::with_capacity(bundles.len());

                if replay_block_txs {
                    // only need to replay the transactions in the block if not all transactions are
                    // to be replayed
                    let transactions = block.transactions_recovered().take(num_txs);

                    // Execute all transactions until index
                    for tx in transactions {
                        let tx_env = eth_api.evm_config().tx_env(tx);
                        let res = eth_api.transact(&mut db, evm_env.clone(), tx_env)?;
                        db.commit(res.state);
                    }
                }

                // Trace all bundles
                let mut bundles = bundles.into_iter().peekable();
                let mut inspector = DebugInspector::new(tracing_options.clone())?;
                while let Some(bundle) = bundles.next() {
                    let mut results = Vec::with_capacity(bundle.transactions.len());
                    let Bundle { transactions, block_override } = bundle;

                    let block_overrides = block_override.map(Box::new);

                    let mut transactions = transactions.into_iter().peekable();
                    while let Some(tx) = transactions.next() {
                        // apply state overrides only once, before the first transaction
                        let state_overrides = state_overrides.take();
                        let overrides = EvmOverrides::new(state_overrides, block_overrides.clone());

                        let (evm_env, tx_env) =
                            eth_api.prepare_call_env(evm_env.clone(), tx, &mut db, overrides)?;

                        let res = eth_api.inspect(
                            &mut db,
                            evm_env.clone(),
                            tx_env.clone(),
                            &mut inspector,
                        )?;
                        let trace = inspector.get_result(
                            None,
                            &tx_env,
                            &evm_env.block_env,
                            &res,
                            &mut db,
                        )?;

                        // If there is more transactions, commit the database
                        // If there is no transactions, but more bundles, commit to the database too
                        if transactions.peek().is_some() || bundles.peek().is_some() {
                            inspector.fuse()?;
                            db.commit(res.state);
                        }
                        results.push(trace);
                    }
                    // Increment block_env number and timestamp for the next bundle
                    evm_env.block_env.inner_mut().number += uint!(1_U256);
                    evm_env.block_env.inner_mut().timestamp += uint!(12_U256);

                    all_bundles.push(results);
                }
                Ok(all_bundles)
            })
            .await
    }

    /// Generates an execution witness for the given block hash. see
    /// [`Self::debug_execution_witness`] for more info.
    pub async fn debug_execution_witness_by_block_hash(
        &self,
        hash: B256,
    ) -> Result<ExecutionWitness, Eth::Error> {
        let this = self.clone();
        let block = this
            .eth_api()
            .recovered_block(hash.into())
            .await?
            .ok_or(EthApiError::HeaderNotFound(hash.into()))?;

        self.debug_execution_witness_for_block(block).await
    }

    /// The `debug_executionWitness` method allows for re-execution of a block with the purpose of
    /// generating an execution witness. The witness comprises of a map of all hashed trie nodes to
    /// their preimages that were required during the execution of the block, including during state
    /// root recomputation.
    pub async fn debug_execution_witness(
        &self,
        block_id: BlockNumberOrTag,
    ) -> Result<ExecutionWitness, Eth::Error> {
        let this = self.clone();
        let block = this
            .eth_api()
            .recovered_block(block_id.into())
            .await?
            .ok_or(EthApiError::HeaderNotFound(block_id.into()))?;

        self.debug_execution_witness_for_block(block).await
    }

    /// Generates an execution witness, using the given recovered block.
    pub async fn debug_execution_witness_for_block(
        &self,
        block: Arc<RecoveredBlock<ProviderBlock<Eth::Provider>>>,
    ) -> Result<ExecutionWitness, Eth::Error> {
        let block_number = block.header().number();

        let (mut exec_witness, lowest_block_number) = self
            .eth_api()
            .spawn_with_state_at_block(block.parent_hash(), move |eth_api, mut db| {
                let block_executor = eth_api.evm_config().executor(&mut db);

                let mut witness_record = ExecutionWitnessRecord::default();

                let _ = block_executor
                    .execute_with_state_closure(&block, |statedb: &State<_>| {
                        witness_record.record_executed_state(statedb);
                    })
                    .map_err(|err| EthApiError::Internal(err.into()))?;

                let ExecutionWitnessRecord { hashed_state, codes, keys, lowest_block_number } =
                    witness_record;

                let state = db
                    .database
                    .0
                    .witness(Default::default(), hashed_state)
                    .map_err(EthApiError::from)?;
                Ok((
                    ExecutionWitness { state, codes, keys, ..Default::default() },
                    lowest_block_number,
                ))
            })
            .await?;

        let smallest = match lowest_block_number {
            Some(smallest) => smallest,
            None => {
                // Return only the parent header, if there were no calls to the
                // BLOCKHASH opcode.
                block_number.saturating_sub(1)
            }
        };

        let range = smallest..block_number;
        exec_witness.headers = self
            .provider()
            .headers_range(range)
            .map_err(EthApiError::from)?
            .into_iter()
            .map(|header| {
                let mut serialized_header = Vec::new();
                header.encode(&mut serialized_header);
                serialized_header.into()
            })
            .collect();

        Ok(exec_witness)
    }

    /// Returns the code associated with a given hash at the specified block ID. If no code is
    /// found, it returns None. If no block ID is provided, it defaults to the latest block.
    pub async fn debug_code_by_hash(
        &self,
        hash: B256,
        block_id: Option<BlockId>,
    ) -> Result<Option<Bytes>, Eth::Error> {
        Ok(self
            .provider()
            .state_by_block_id(block_id.unwrap_or_default())
            .map_err(Eth::Error::from_eth_err)?
            .bytecode_by_hash(&hash)
            .map_err(Eth::Error::from_eth_err)?
            .map(|b| b.original_bytes()))
    }

    /// Returns the state root of the `HashedPostState` on top of the state for the given block with
    /// trie updates.
    async fn debug_state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
        block_id: Option<BlockId>,
    ) -> Result<(B256, TrieUpdates), Eth::Error> {
        self.inner
            .eth_api
            .spawn_blocking_io(move |this| {
                let state = this
                    .provider()
                    .state_by_block_id(block_id.unwrap_or_default())
                    .map_err(Eth::Error::from_eth_err)?;
                state.state_root_with_updates(hashed_state).map_err(Eth::Error::from_eth_err)
            })
            .await
    }

    /// Replays the given block with [`StorageDiffInspector`] and returns the storage diffs for each
    /// executed transaction.
    #[cfg(test)]
    async fn run_with_storage_diff_inspector_for_tests(
        &self,
        block_id: BlockId,
        highest_tx_index: Option<u64>,
    ) -> StorageDiffs {
        self.run_with_storage_diff_inspector(block_id, highest_tx_index)
            .await
            .expect("run_with_storage_diff_inspector should succeed in tests")
    }
}

#[async_trait]
impl<Eth> DebugApiServer<RpcTxReq<Eth::NetworkTypes>> for DebugApi<Eth>
where
    Eth: EthApiTypes + EthTransactions + TraceExt + 'static,
{
    /// Handler for `debug_getRawHeader`
    async fn raw_header(&self, block_id: BlockId) -> RpcResult<Bytes> {
        let header = match block_id {
            BlockId::Hash(hash) => self.provider().header(hash.into()).to_rpc_result()?,
            BlockId::Number(number_or_tag) => {
                let number = self
                    .provider()
                    .convert_block_number(number_or_tag)
                    .to_rpc_result()?
                    .ok_or_else(|| {
                    internal_rpc_err("Pending block not supported".to_string())
                })?;
                self.provider().header_by_number(number).to_rpc_result()?
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
            .provider()
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
            .provider()
            .block_with_senders_by_id(block_id, TransactionVariant::NoHash)
            .to_rpc_result()?
            .unwrap_or_default();
        Ok(block.into_transactions_recovered().map(|tx| tx.encoded_2718().into()).collect())
    }

    /// Handler for `debug_getRawReceipts`
    async fn raw_receipts(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>> {
        Ok(self
            .provider()
            .receipts_by_block_id(block_id)
            .to_rpc_result()?
            .unwrap_or_default()
            .into_iter()
            .map(|receipt| ReceiptWithBloom::from(receipt).encoded_2718().into())
            .collect())
    }

    /// Handler for `debug_getBadBlocks`
    async fn bad_blocks(&self) -> RpcResult<Vec<RpcBlock>> {
        Ok(vec![])
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

    /// Handler for `debug_traceCall`
    async fn debug_trace_call(
        &self,
        request: RpcTxReq<Eth::NetworkTypes>,
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
        bundles: Vec<Bundle<RpcTxReq<Eth::NetworkTypes>>>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<Vec<Vec<GethTrace>>> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_trace_call_many(self, bundles, state_context, opts).await.map_err(Into::into)
    }

    /// Handler for `debug_executionWitness`
    async fn debug_execution_witness(
        &self,
        block: BlockNumberOrTag,
    ) -> RpcResult<ExecutionWitness> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_execution_witness(self, block).await.map_err(Into::into)
    }

    /// Handler for `debug_executionWitnessByBlockHash`
    async fn debug_execution_witness_by_block_hash(
        &self,
        hash: B256,
    ) -> RpcResult<ExecutionWitness> {
        let _permit = self.acquire_trace_permit().await;
        Self::debug_execution_witness_by_block_hash(self, hash).await.map_err(Into::into)
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

    async fn debug_chain_config(&self) -> RpcResult<ChainConfig> {
        Ok(self.provider().chain_spec().genesis().config.clone())
    }

    async fn debug_chaindb_property(&self, _property: String) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_code_by_hash(
        &self,
        hash: B256,
        block_id: Option<BlockId>,
    ) -> RpcResult<Option<Bytes>> {
        Self::debug_code_by_hash(self, hash, block_id).await.map_err(Into::into)
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

    /// `debug_db_get` - database key lookup
    ///
    /// Currently supported:
    /// * Contract bytecode associated with a code hash. The key format is: `<0x63><code_hash>`
    ///     * Prefix byte: 0x63 (required)
    ///     * Code hash: 32 bytes
    ///   Must be provided as either:
    ///     * Hex string: "0x63..." (66 hex characters after 0x)
    ///     * Raw byte string: raw byte string (33 bytes)
    ///   See Geth impl: <https://github.com/ethereum/go-ethereum/blob/737ffd1bf0cbee378d0111a5b17ae4724fb2216c/core/rawdb/schema.go#L120>
    async fn debug_db_get(&self, key: String) -> RpcResult<Option<Bytes>> {
        let key_bytes = if key.starts_with("0x") {
            decode(&key).map_err(|_| EthApiError::InvalidParams("Invalid hex key".to_string()))?
        } else {
            key.into_bytes()
        };

        if key_bytes.len() != 33 {
            return Err(EthApiError::InvalidParams(format!(
                "Key must be 33 bytes, got {}",
                key_bytes.len()
            ))
            .into());
        }
        if key_bytes[0] != 0x63 {
            return Err(EthApiError::InvalidParams("Key prefix must be 0x63".to_string()).into());
        }

        let code_hash = B256::from_slice(&key_bytes[1..33]);

        // No block ID is provided, so it defaults to the latest block
        self.debug_code_by_hash(code_hash, None).await.map_err(Into::into)
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

    async fn debug_state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
        block_id: Option<BlockId>,
    ) -> RpcResult<(B256, TrieUpdates)> {
        Self::debug_state_root_with_updates(self, hashed_state, block_id).await.map_err(Into::into)
    }

    async fn debug_stop_cpu_profile(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_stop_go_trace(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn debug_storage_range_at(
        &self,
        block_hash: B256,
        tx_idx: usize,
        contract_address: Address,
        key_start: B256,
        max_result: u64,
    ) -> RpcResult<RpcStorageRangeResult> {
        Self::storage_range_at(self, block_hash, tx_idx, contract_address, key_start, max_result)
            .await
            .map_err(Into::into)
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

impl<Eth> std::fmt::Debug for DebugApi<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}

impl<Eth> Clone for DebugApi<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct DebugApiInner<Eth> {
    /// The implementation of `eth` API
    eth_api: Eth,
    // restrict the number of concurrent calls to blocking calls
    blocking_task_guard: BlockingTaskGuard,
}

fn convert_storage_range(range: ProviderStorageRangeResult) -> RpcStorageRangeResult {
    let mut storage = BTreeMap::new();
    for entry in range.slots {
        let value = B256::from(entry.value.to_be_bytes());
        storage.insert(entry.key, StorageResult { key: entry.key, value });
    }

    RpcStorageRangeResult { storage: StorageMap(storage), next_key: range.next_key }
}

/// Inspector for the `debug` API
///
/// This inspector is used to trace the execution of a transaction or call and supports all variants
/// of [`GethDebugTracerType`].
///
/// This inspector can be re-used for tracing multiple transactions. This is supported by
/// requiring caller to invoke [`DebugInspector::fuse`] after each transaction. See method
/// documentation for more details.
enum DebugInspector {
    FourByte(FourByteInspector),
    CallTracer(TracingInspector, CallConfig),
    PreStateTracer(TracingInspector, PreStateConfig),
    Noop(NoOpInspector),
    Mux(MuxInspector, MuxConfig),
    FlatCallTracer(TracingInspector),
    Default(TracingInspector, GethDefaultTracingOptions),
    #[cfg(feature = "js-tracer")]
    Js(Box<revm_inspectors::tracing::js::JsInspector>, String, serde_json::Value),
}

impl DebugInspector {
    /// Create a new `DebugInspector` from the given tracing options.
    fn new(opts: GethDebugTracingOptions) -> Result<Self, EthApiError> {
        let GethDebugTracingOptions { config, tracer, tracer_config, .. } = opts;

        let this = if let Some(tracer) = tracer {
            #[allow(unreachable_patterns)]
            match tracer {
                GethDebugTracerType::BuiltInTracer(tracer) => match tracer {
                    GethDebugBuiltInTracerType::FourByteTracer => {
                        Self::FourByte(FourByteInspector::default())
                    }
                    GethDebugBuiltInTracerType::CallTracer => {
                        let config = tracer_config
                            .into_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        Self::CallTracer(
                            TracingInspector::new(TracingInspectorConfig::from_geth_call_config(
                                &config,
                            )),
                            config,
                        )
                    }
                    GethDebugBuiltInTracerType::PreStateTracer => {
                        let config = tracer_config
                            .into_pre_state_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        Self::PreStateTracer(
                            TracingInspector::new(
                                TracingInspectorConfig::from_geth_prestate_config(&config),
                            ),
                            config,
                        )
                    }
                    GethDebugBuiltInTracerType::NoopTracer => Self::Noop(NoOpInspector),
                    GethDebugBuiltInTracerType::MuxTracer => {
                        let config = tracer_config
                            .into_mux_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        Self::Mux(MuxInspector::try_from_config(config.clone())?, config)
                    }
                    GethDebugBuiltInTracerType::FlatCallTracer => {
                        let flat_call_config = tracer_config
                            .into_flat_call_config()
                            .map_err(|_| EthApiError::InvalidTracerConfig)?;

                        Self::FlatCallTracer(TracingInspector::new(
                            TracingInspectorConfig::from_flat_call_config(&flat_call_config),
                        ))
                    }
                    _ => {
                        // Note: this match is non-exhaustive in case we need to add support for
                        // additional tracers
                        return Err(EthApiError::Unsupported("unsupported tracer"))
                    }
                },
                #[cfg(not(feature = "js-tracer"))]
                GethDebugTracerType::JsTracer(_) => {
                    return Err(EthApiError::Unsupported("JS Tracer is not enabled"))
                }
                #[cfg(feature = "js-tracer")]
                GethDebugTracerType::JsTracer(code) => {
                    let config = tracer_config.into_json();
                    Self::Js(
                        revm_inspectors::tracing::js::JsInspector::new(
                            code.clone(),
                            config.clone(),
                        )?
                        .into(),
                        code,
                        config,
                    )
                }
                _ => {
                    // Note: this match is non-exhaustive in case we need to add support for
                    // additional tracers
                    return Err(EthApiError::Unsupported("unsupported tracer"))
                }
            }
        } else {
            Self::Default(
                TracingInspector::new(TracingInspectorConfig::from_geth_config(&config)),
                config,
            )
        };

        Ok(this)
    }

    /// Prepares inspector for executing the next transaction. This will remove any state from
    /// previous transactions.
    fn fuse(&mut self) -> Result<(), EthApiError> {
        match self {
            Self::FourByte(inspector) => {
                std::mem::take(inspector);
            }
            Self::CallTracer(inspector, _) |
            Self::PreStateTracer(inspector, _) |
            Self::FlatCallTracer(inspector) |
            Self::Default(inspector, _) => inspector.fuse(),
            Self::Noop(_) => {}
            Self::Mux(inspector, config) => {
                *inspector = MuxInspector::try_from_config(config.clone())?
            }
            #[cfg(feature = "js-tracer")]
            Self::Js(inspector, code, config) => {
                *inspector =
                    revm_inspectors::tracing::js::JsInspector::new(code.clone(), config.clone())?
                        .into();
            }
        }

        Ok(())
    }

    /// Should be invoked after each transaction to obtain the resulting [`GethTrace`].
    fn get_result(
        &mut self,
        tx_context: Option<TransactionContext>,
        tx_env: &impl revm::context::Transaction,
        block_env: &impl revm::context::Block,
        res: &ResultAndState<impl HaltReasonTr>,
        db: &mut StateCacheDb,
    ) -> Result<GethTrace, EthApiError> {
        let tx_info = TransactionInfo {
            hash: tx_context.as_ref().map(|c| c.tx_hash).unwrap_or_default(),
            index: tx_context.as_ref().map(|c| c.tx_index.map(|i| i as u64)).unwrap_or_default(),
            block_hash: tx_context.as_ref().map(|c| c.block_hash).unwrap_or_default(),
            block_number: Some(block_env.number().saturating_to()),
            base_fee: Some(block_env.basefee()),
        };

        let res = match self {
            Self::FourByte(inspector) => FourByteFrame::from(&*inspector).into(),
            Self::CallTracer(inspector, config) => {
                inspector.set_transaction_gas_limit(tx_env.gas_limit());
                inspector.geth_builder().geth_call_traces(*config, res.result.gas_used()).into()
            }
            Self::PreStateTracer(inspector, config) => {
                inspector.set_transaction_gas_limit(tx_env.gas_limit());
                inspector.geth_builder().geth_prestate_traces(res, config, db)?.into()
            }
            Self::Noop(_) => NoopFrame::default().into(),
            Self::Mux(inspector, _) => inspector.try_into_mux_frame(res, db, tx_info)?.into(),
            Self::FlatCallTracer(inspector) => {
                inspector.set_transaction_gas_limit(tx_env.gas_limit());
                inspector
                    .clone()
                    .into_parity_builder()
                    .into_localized_transaction_traces(tx_info)
                    .into()
            }
            Self::Default(inspector, config) => {
                inspector.set_transaction_gas_limit(tx_env.gas_limit());
                inspector
                    .geth_builder()
                    .geth_traces(
                        res.result.gas_used(),
                        res.result.output().unwrap_or_default().clone(),
                        *config,
                    )
                    .into()
            }
            #[cfg(feature = "js-tracer")]
            Self::Js(inspector, _, _) => {
                inspector.set_transaction_context(tx_context.unwrap_or_default());
                let res = inspector.json_result(res.clone(), tx_env, block_env, db)?;

                GethTrace::JS(res)
            }
        };

        Ok(res)
    }
}

macro_rules! delegate {
    ($self:expr => $insp:ident.$method:ident($($arg:expr),*)) => {
        match $self {
            Self::FourByte($insp) => Inspector::<CTX>::$method($insp, $($arg),*),
            Self::CallTracer($insp, _) => Inspector::<CTX>::$method($insp, $($arg),*),
            Self::PreStateTracer($insp, _) => Inspector::<CTX>::$method($insp, $($arg),*),
            Self::FlatCallTracer($insp) => Inspector::<CTX>::$method($insp, $($arg),*),
            Self::Default($insp, _) => Inspector::<CTX>::$method($insp, $($arg),*),
            Self::Noop($insp) => Inspector::<CTX>::$method($insp, $($arg),*),
            Self::Mux($insp, _) => Inspector::<CTX>::$method($insp, $($arg),*),
            #[cfg(feature = "js-tracer")]
            Self::Js($insp, _, _) => Inspector::<CTX>::$method($insp, $($arg),*),
        }
    };
}

impl<CTX> Inspector<CTX> for DebugInspector
where
    CTX: ContextTr<Journal: JournalExt, Db: DatabaseRef>,
{
    fn initialize_interp(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        delegate!(self => inspector.initialize_interp(interp, context))
    }

    fn step(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        delegate!(self => inspector.step(interp, context))
    }

    fn step_end(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        delegate!(self => inspector.step_end(interp, context))
    }

    fn log(&mut self, context: &mut CTX, log: Log) {
        delegate!(self => inspector.log(context, log))
    }

    fn log_full(&mut self, interp: &mut Interpreter, context: &mut CTX, log: Log) {
        delegate!(self => inspector.log_full(interp, context, log))
    }

    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        delegate!(self => inspector.call(context, inputs))
    }

    fn call_end(&mut self, context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        delegate!(self => inspector.call_end(context, inputs, outcome))
    }

    fn create(&mut self, context: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        delegate!(self => inspector.create(context, inputs))
    }

    fn create_end(
        &mut self,
        context: &mut CTX,
        inputs: &CreateInputs,
        outcome: &mut CreateOutcome,
    ) {
        delegate!(self => inspector.create_end(context, inputs, outcome))
    }

    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        delegate!(self => inspector.selfdestruct(contract, target, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::{helpers::types::EthRpcConverter, EthApi};
    use alloy_consensus::{Header, TxLegacy};
    use alloy_eips::BlockId;
    use alloy_primitives::{Address, Bytes, StorageKey, StorageValue, TxKind, B256, U256};
    use reth_chainspec::ChainSpec;
    use reth_db_api::models::StoredBlockBodyIndices;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives_traits::{crypto::secp256k1::public_key_to_address, StorageEntry};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_rpc_eth_api::node::RpcNodeCoreAdapter;
    use reth_testing_utils::generators::sign_tx_with_key_pair;
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use secp256k1::{Keypair, Secp256k1, SecretKey};

    type TestRpcNode = RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>;
    type TestEthApi = EthApi<TestRpcNode, EthRpcConverter<ChainSpec>>;

    #[derive(Clone)]
    struct ContractSpec {
        address: Address,
        slot: u8,
        value: u8,
    }

    fn make_sstore_code(slot: u8, value: u8) -> Bytes {
        Bytes::from(vec![0x60, value, 0x60, slot, 0x55, 0x00])
    }

    fn make_call_tx(
        keypair: &Keypair,
        nonce: u64,
        to: Address,
    ) -> reth_ethereum_primitives::TransactionSigned {
        let tx = reth_ethereum_primitives::Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: 1,
            gas_limit: 100_000,
            to: TxKind::Call(to),
            value: U256::ZERO,
            input: Bytes::new(),
        });
        sign_tx_with_key_pair(*keypair, tx)
    }

    fn build_debug_api_with_contract_writes(
        writes: &[(u8, u8)],
    ) -> (DebugApi<TestEthApi>, BlockId, Vec<ContractSpec>) {
        let provider = MockEthProvider::default();
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x11; 32]).expect("valid secret key");
        let keypair = Keypair::from_secret_key(&secp, &secret_key);
        let sender = public_key_to_address(keypair.public_key());
        let mut accounts = Vec::new();
        accounts.push((sender, ExtendedAccount::new(0, U256::from(1_000_000_000_000_000_000u128))));

        let mut specs = Vec::new();
        let mut txs = Vec::new();
        for (idx, (slot, value)) in writes.iter().copied().enumerate() {
            let address = Address::repeat_byte(0x80 + idx as u8);
            let account =
                ExtendedAccount::new(0, U256::ZERO).with_bytecode(make_sstore_code(slot, value));
            accounts.push((address, account));
            specs.push(ContractSpec { address, slot, value });
            txs.push(make_call_tx(&keypair, idx as u64, address));
        }

        provider.extend_accounts(accounts);

        let mut header = Header {
            number: 1,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1),
            ..Header::default()
        };
        header.parent_hash = B256::with_last_byte(0xAA);
        let block = reth_ethereum_primitives::Block {
            header: header.clone(),
            body: reth_ethereum_primitives::BlockBody {
                transactions: txs.clone(),
                ..Default::default()
            },
        };
        let block_hash = header.hash_slow();
        provider.add_block(block_hash, block);
        provider.add_block_body_indices(
            header.number,
            StoredBlockBodyIndices { first_tx_num: 0, tx_count: txs.len() as u64 },
        );

        let evm_config = EthEvmConfig::new(provider.chain_spec());
        let eth_api =
            EthApi::builder(provider, testing_pool(), NoopNetwork::default(), evm_config).build();

        let debug_api = DebugApi::new(eth_api, BlockingTaskGuard::new(4));
        (debug_api, BlockId::Hash(block_hash.into()), specs)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collects_storage_diffs_for_block() {
        let (api, block_id, specs) = build_debug_api_with_contract_writes(&[(1, 0x2A)]);
        let diffs = api.run_with_storage_diff_inspector_for_tests(block_id, None).await;
        assert_eq!(diffs.len(), 1);

        let tx_diff = &diffs[0];
        let spec = &specs[0];
        let key = StorageKey::from(U256::from(spec.slot));
        let value = StorageValue::from(U256::from(spec.value));
        let slots = tx_diff.get(&spec.address).expect("contract entry must exist");
        assert_eq!(slots.get(&key), Some(&value));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn respects_tx_index_limit() {
        let (api, block_id, specs) = build_debug_api_with_contract_writes(&[(1, 0x10), (2, 0x20)]);

        let capped = api.run_with_storage_diff_inspector_for_tests(block_id, Some(0)).await;
        assert_eq!(capped.len(), 1);
        let first_contract_diff = capped[0].get(&specs[0].address).expect("first tx diff missing");
        let slot = StorageKey::from(U256::from(specs[0].slot));
        let value = StorageValue::from(U256::from(specs[0].value));
        assert_eq!(first_contract_diff.get(&slot), Some(&value));
        assert!(!capped[0].contains_key(&specs[1].address));

        let full = api.run_with_storage_diff_inspector_for_tests(block_id, None).await;
        assert_eq!(full.len(), specs.len());

        for (idx, spec) in specs.iter().enumerate() {
            let entry = full[idx].get(&spec.address).expect("expected contract diff");
            let slot = StorageKey::from(U256::from(spec.slot));
            let value = StorageValue::from(U256::from(spec.value));
            assert_eq!(entry.get(&slot), Some(&value));
        }
    }

    #[test]
    fn converts_storage_range_result() {
        let entry = StorageEntry::new(B256::from(U256::from(1)), U256::from(2));
        let range = ProviderStorageRangeResult {
            slots: vec![entry],
            next_key: Some(StorageKey::from(B256::from(U256::from(3)))),
        };

        let rpc = convert_storage_range(range);
        assert_eq!(rpc.next_key, Some(B256::from(U256::from(3))));
        assert_eq!(rpc.storage.len(), 1);
        let (key, slot) = rpc.storage.0.iter().next().unwrap();
        assert_eq!(key, &entry.key);
        assert_eq!(slot.key, entry.key);
        assert_eq!(slot.value, B256::from(entry.value.to_be_bytes()));
    }
}
