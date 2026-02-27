use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eip7928::BlockAccessList;
use alloy_eips::{eip2718::Encodable2718, BlockId, BlockNumberOrTag};
use alloy_evm::env::BlockEnvironment;
use alloy_genesis::ChainConfig;
use alloy_primitives::{hex::decode, uint, Address, Bytes, B256, U64};
use alloy_rlp::{Decodable, Encodable};
use alloy_rpc_types::BlockTransactionsKind;
use alloy_rpc_types_debug::ExecutionWitness;
use alloy_rpc_types_eth::{state::EvmOverrides, BlockError, Bundle, StateContext};
use alloy_rpc_types_trace::geth::{
    BlockTraceResult, GethDebugTracingCallOptions, GethDebugTracingOptions, GethTrace, TraceResult,
};
use async_trait::async_trait;
use futures::Stream;
use jsonrpsee::core::RpcResult;
use parking_lot::RwLock;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_engine_primitives::ConsensusEngineEvent;
use reth_errors::RethError;
use reth_evm::{execute::Executor, ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::{
    Block as BlockTrait, BlockBody, BlockTy, ReceiptWithBloom, RecoveredBlock,
};
use reth_revm::{db::State, witness::ExecutionWitnessRecord};
use reth_rpc_api::DebugApiServer;
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_api::{
    helpers::{EthTransactions, TraceExt},
    FromEthApiError, RpcConvert, RpcNodeCore,
};
use reth_rpc_eth_types::EthApiError;
use reth_rpc_server_types::{result::internal_rpc_err, ToRpcResult};
use reth_storage_api::{
    BlockIdReader, BlockReaderIdExt, HeaderProvider, ProviderBlock, ReceiptProviderIdExt,
    StateProofProvider, StateProviderFactory, StateRootProvider, TransactionVariant,
};
use reth_tasks::{pool::BlockingTaskGuard, Runtime};
use reth_trie_common::{updates::TrieUpdates, HashedPostState};
use revm::DatabaseCommit;
use revm_inspectors::tracing::{DebugInspector, TransactionContext};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::{AcquireError, OwnedSemaphorePermit};
use tokio_stream::StreamExt;

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
pub struct DebugApi<Eth: RpcNodeCore> {
    inner: Arc<DebugApiInner<Eth>>,
}

impl<Eth> DebugApi<Eth>
where
    Eth: RpcNodeCore,
{
    /// Create a new instance of the [`DebugApi`]
    pub fn new(
        eth_api: Eth,
        blocking_task_guard: BlockingTaskGuard,
        executor: &Runtime,
        mut stream: impl Stream<Item = ConsensusEngineEvent<Eth::Primitives>> + Send + Unpin + 'static,
    ) -> Self {
        let bad_block_store = BadBlockStore::default();
        let inner = Arc::new(DebugApiInner {
            eth_api,
            blocking_task_guard,
            bad_block_store: bad_block_store.clone(),
        });

        // Spawn a task caching bad blocks
        executor.spawn_task(async move {
            while let Some(event) = stream.next().await {
                if let ConsensusEngineEvent::InvalidBlock(block) = event &&
                    let Ok(recovered) =
                        RecoveredBlock::try_recover_sealed(block.as_ref().clone())
                {
                    bad_block_store.insert(recovered);
                }
            }
        });

        Self { inner }
    }

    /// Access the underlying `Eth` API.
    pub fn eth_api(&self) -> &Eth {
        &self.inner.eth_api
    }

    /// Access the underlying provider.
    pub fn provider(&self) -> &Eth::Provider {
        self.inner.eth_api.provider()
    }
}

// === impl DebugApi ===

impl<Eth> DebugApi<Eth>
where
    Eth: TraceExt,
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

                eth_api.apply_pre_execution_changes(&block, &mut db)?;

                let mut transactions = block.transactions_recovered().enumerate().peekable();
                let mut inspector = DebugInspector::new(opts).map_err(Eth::Error::from_eth_err)?;
                while let Some((index, tx)) = transactions.next() {
                    let tx_hash = *tx.tx_hash();
                    let tx_env = eth_api.evm_config().tx_env(tx);

                    let res = eth_api.inspect(
                        &mut db,
                        evm_env.clone(),
                        tx_env.clone(),
                        &mut inspector,
                    )?;
                    let result = inspector
                        .get_result(
                            Some(TransactionContext {
                                block_hash: Some(block.hash()),
                                tx_hash: Some(tx_hash),
                                tx_index: Some(index),
                            }),
                            &tx_env,
                            &evm_env.block_env,
                            &res,
                            &mut db,
                        )
                        .map_err(Eth::Error::from_eth_err)?;

                    results.push(TraceResult::Success { result, tx_hash: Some(tx_hash) });
                    if transactions.peek().is_some() {
                        inspector.fuse().map_err(Eth::Error::from_eth_err)?;
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
                block.body().recover_signers()
            } else {
                block.body().recover_signers_unchecked()
            }
            .map_err(Eth::Error::from_eth_err)?;

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

                eth_api.apply_pre_execution_changes(&block, &mut db)?;

                // replay all transactions prior to the targeted transaction
                let index = eth_api.replay_transactions_until(
                    &mut db,
                    evm_env.clone(),
                    block_txs,
                    *tx.tx_hash(),
                )?;

                let tx_env = eth_api.evm_config().tx_env(&tx);

                let mut inspector = DebugInspector::new(opts).map_err(Eth::Error::from_eth_err)?;
                let res =
                    eth_api.inspect(&mut db, evm_env.clone(), tx_env.clone(), &mut inspector)?;
                let trace = inspector
                    .get_result(
                        Some(TransactionContext {
                            block_hash: Some(block_hash),
                            tx_index: Some(index),
                            tx_hash: Some(*tx.tx_hash()),
                        }),
                        &tx_env,
                        &evm_env.block_env,
                        &res,
                        &mut db,
                    )
                    .map_err(Eth::Error::from_eth_err)?;

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
                let mut inspector =
                    DebugInspector::new(tracing_options).map_err(Eth::Error::from_eth_err)?;
                let res = this.eth_api().inspect(
                    &mut *db,
                    evm_env.clone(),
                    tx_env.clone(),
                    &mut inspector,
                )?;
                let trace = inspector
                    .get_result(None, &tx_env, &evm_env.block_env, &res, db)
                    .map_err(Eth::Error::from_eth_err)?;
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
                eth_api.apply_pre_execution_changes(&block, &mut db)?;

                // 2. replay the required number of transactions
                eth_api.replay_transactions_until(
                    &mut db,
                    evm_env.clone(),
                    block.transactions_recovered(),
                    *block.body().transactions()[tx_index].tx_hash(),
                )?;

                // 3. now execute the trace call on this state
                let (evm_env, tx_env) =
                    eth_api.prepare_call_env(evm_env, call, &mut db, overrides)?;

                let mut inspector =
                    DebugInspector::new(tracing_options).map_err(Eth::Error::from_eth_err)?;
                let res =
                    eth_api.inspect(&mut db, evm_env.clone(), tx_env.clone(), &mut inspector)?;
                let trace = inspector
                    .get_result(None, &tx_env, &evm_env.block_env, &res, &mut db)
                    .map_err(Eth::Error::from_eth_err)?;

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
                    eth_api.apply_pre_execution_changes(&block, &mut db)?;

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
                let mut inspector = DebugInspector::new(tracing_options.clone())
                    .map_err(Eth::Error::from_eth_err)?;
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
                        let trace = inspector
                            .get_result(None, &tx_env, &evm_env.block_env, &res, &mut db)
                            .map_err(Eth::Error::from_eth_err)?;

                        // If there is more transactions, commit the database
                        // If there is no transactions, but more bundles, commit to the database too
                        if transactions.peek().is_some() || bundles.peek().is_some() {
                            inspector.fuse().map_err(Eth::Error::from_eth_err)?;
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
}

#[async_trait]
impl<Eth> DebugApiServer<RpcTxReq<Eth::NetworkTypes>> for DebugApi<Eth>
where
    Eth: EthTransactions + TraceExt,
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
        let block: RecoveredBlock<BlockTy<Eth::Primitives>> = self
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
    async fn bad_blocks(&self) -> RpcResult<Vec<serde_json::Value>> {
        let blocks = self.inner.bad_block_store.all();
        let mut bad_blocks = Vec::with_capacity(blocks.len());

        #[derive(Serialize, Deserialize)]
        struct BadBlockSerde<T> {
            block: T,
            hash: B256,
            rlp: Bytes,
        }

        for block in blocks {
            let rlp = alloy_rlp::encode(block.sealed_block()).into();
            let hash = block.hash();

            let block = block
                .clone_into_rpc_block(
                    BlockTransactionsKind::Full,
                    |tx, tx_info| self.eth_api().converter().fill(tx, tx_info),
                    |header, size| self.eth_api().converter().convert_header(header, size),
                )
                .map_err(|err| Eth::Error::from(err).into())?;

            let bad_block = serde_json::to_value(BadBlockSerde { block, hash, rlp })
                .map_err(|err| EthApiError::other(internal_rpc_err(err.to_string())))?;

            bad_blocks.push(bad_block);
        }

        Ok(bad_blocks)
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

    async fn debug_get_block_access_list(&self, _block_id: BlockId) -> RpcResult<BlockAccessList> {
        Err(internal_rpc_err("unimplemented"))
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

    async fn debug_set_head(&self, _number: U64) -> RpcResult<()> {
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

impl<Eth: RpcNodeCore> std::fmt::Debug for DebugApi<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}

impl<Eth: RpcNodeCore> Clone for DebugApi<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct DebugApiInner<Eth: RpcNodeCore> {
    /// The implementation of `eth` API
    eth_api: Eth,
    // restrict the number of concurrent calls to blocking calls
    blocking_task_guard: BlockingTaskGuard,
    /// Cache for bad blocks.
    bad_block_store: BadBlockStore<BlockTy<Eth::Primitives>>,
}

/// A bounded, deduplicating store of recently observed bad blocks.
#[derive(Clone, Debug)]
struct BadBlockStore<B: BlockTrait> {
    inner: Arc<RwLock<VecDeque<Arc<RecoveredBlock<B>>>>>,
    limit: usize,
}

impl<B: BlockTrait> BadBlockStore<B> {
    /// Creates a new store with the given capacity.
    fn new(limit: usize) -> Self {
        Self { inner: Arc::new(RwLock::new(VecDeque::with_capacity(limit))), limit }
    }

    /// Inserts a recovered block, keeping only the most recent `limit` entries and deduplicating
    /// by block hash.
    fn insert(&self, block: RecoveredBlock<B>) {
        let hash = block.hash();
        let mut guard = self.inner.write();

        // skip if we already recorded this bad block , and keep original ordering
        if guard.iter().any(|b| b.hash() == hash) {
            return;
        }
        guard.push_back(Arc::new(block));

        while guard.len() > self.limit {
            guard.pop_front();
        }
    }

    /// Returns all cached bad blocks ordered from newest to oldest.
    fn all(&self) -> Vec<Arc<RecoveredBlock<B>>> {
        let guard = self.inner.read();
        guard.iter().rev().cloned().collect()
    }
}

impl<B: BlockTrait> Default for BadBlockStore<B> {
    fn default() -> Self {
        Self::new(64)
    }
}
