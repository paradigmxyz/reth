//! Database access for `eth_` transaction RPC methods. Loads transaction and receipt data w.r.t.
//! network.

use std::sync::Arc;

use reth_evm::ConfigureEvm;
use reth_primitives::{
    revm::env::fill_block_env_with_coinbase, Address, BlockId, Bytes,
    FromRecoveredPooledTransaction, Header, IntoRecoveredTransaction, Receipt, SealedBlock,
    SealedBlockWithSenders, TransactionMeta, TransactionSigned, TxHash, B256, U256,
};
use reth_provider::{BlockReaderIdExt, ReceiptProvider, StateProviderBox, TransactionsProvider};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_types::{AnyTransactionReceipt, TransactionInfo, TransactionRequest};
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use revm::{
    db::CacheDB,
    primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, EvmState, ExecutionResult,
        ResultAndState,
    },
    GetInspector, Inspector,
};
use revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use revm_primitives::{
    db::{Database, DatabaseRef},
    SpecId,
};

use crate::{
    eth::{
        api::{BuildReceipt, LoadState, SpawnBlocking},
        cache::EthStateCache,
        error::{EthApiError, EthResult, RpcInvalidTransactionError},
        revm_utils::{EvmOverrides, FillableTransaction},
        traits::RawTransactionForwarder,
        utils::recover_raw_transaction,
        TransactionSource,
    },
    EthApiSpec,
};

/// Helper alias type for the state's [`CacheDB`]
pub type StateCacheDB = CacheDB<StateProviderDatabase<StateProviderBox>>;

/// Commonly used transaction related functions for the [`EthApiServer`](crate::EthApi) trait in
/// the `eth_` namespace.
///
/// This includes utilities for transaction tracing, transacting and inspection.
///
/// Async functions that are spawned onto the
/// [`BlockingTaskPool`](reth_tasks::pool::BlockingTaskPool) begin with `spawn_`
///
/// ## Calls
///
/// There are subtle differences between when transacting [`TransactionRequest`]:
///
/// The endpoints `eth_call` and `eth_estimateGas` and `eth_createAccessList` should always
/// __disable__ the base fee check in the [`EnvWithHandlerCfg`]
/// [`Cfg`](revm_primitives::CfgEnvWithHandlerCfg).
///
/// The behaviour for tracing endpoints is not consistent across clients.
/// Geth also disables the basefee check for tracing: <https://github.com/ethereum/go-ethereum/blob/bc0b87ca196f92e5af49bd33cc190ef0ec32b197/eth/tracers/api.go#L955-L955>
/// Erigon does not: <https://github.com/ledgerwatch/erigon/blob/aefb97b07d1c4fd32a66097a24eddd8f6ccacae0/turbo/transactions/tracing.go#L209-L209>
///
/// See also <https://github.com/paradigmxyz/reth/issues/6240>
///
/// This implementation follows the behaviour of Geth and disables the basefee check for tracing.
#[async_trait::async_trait]
pub trait EthTransactions: Send + Sync {
    /// Transaction pool with pending transactions.
    type Pool: TransactionPool;

    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> &impl BlockReaderIdExt;

    /// Returns a handle for reading data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn cache(&self) -> &EthStateCache;

    /// Returns a handle for reading data from pool.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pool(&self) -> &Self::Pool;

    /// Returns a handle for forwarding received raw transactions.
    ///
    /// Access to transaction forwarder in default (L1) trait method implementations.
    fn raw_tx_forwarder(&self) -> &Option<Arc<dyn RawTransactionForwarder>>;

    /// Returns a handle for reading evm config.
    ///
    /// Data access in default (L1) trait method implementations.
    fn evm_config(&self) -> &impl ConfigureEvm;

    /// Executes the [EnvWithHandlerCfg] against the given [Database] without committing state
    /// changes.
    fn transact<DB>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        DB: Database,
        <DB as Database>::Error: Into<EthApiError>,
    {
        let mut evm = self.evm_config().evm_with_env(db, env);
        let res = evm.transact()?;
        let (_, env) = evm.into_db_and_env_with_handler_cfg();
        Ok((res, env))
    }
    /// Executes the [EnvWithHandlerCfg] against the given [Database] without committing state
    /// changes.
    fn inspect<DB, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        DB: Database,
        <DB as Database>::Error: Into<EthApiError>,
        I: GetInspector<DB>,
    {
        self.inspect_and_return_db(db, env, inspector).map(|(res, env, _)| (res, env))
    }

    /// Same as [Self::inspect] but also returns the database again.
    ///
    /// Even though [Database] is also implemented on `&mut`
    /// this is still useful if there are certain trait bounds on the Inspector's database generic
    /// type
    fn inspect_and_return_db<DB, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg, DB)>
    where
        DB: Database,
        <DB as Database>::Error: Into<EthApiError>,
        I: GetInspector<DB>,
    {
        let mut evm = self.evm_config().evm_with_env_and_inspector(db, env, inspector);
        let res = evm.transact()?;
        let (db, env) = evm.into_db_and_env_with_handler_cfg();
        Ok((res, env, db))
    }

    /// Replays all the transactions until the target transaction is found.
    ///
    /// All transactions before the target transaction are executed and their changes are written to
    /// the _runtime_ db ([CacheDB]).
    ///
    /// Note: This assumes the target transaction is in the given iterator.
    /// Returns the index of the target transaction in the given iterator.
    fn replay_transactions_until<DB, I, Tx>(
        &self,
        db: &mut CacheDB<DB>,
        cfg: CfgEnvWithHandlerCfg,
        block_env: BlockEnv,
        transactions: I,
        target_tx_hash: B256,
    ) -> Result<usize, EthApiError>
    where
        DB: DatabaseRef,
        EthApiError: From<<DB as DatabaseRef>::Error>,
        I: IntoIterator<Item = Tx>,
        Tx: FillableTransaction,
    {
        let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default());

        let mut evm = self.evm_config().evm_with_env(db, env);
        let mut index = 0;
        for tx in transactions {
            if tx.hash() == target_tx_hash {
                // reached the target transaction
                break
            }

            tx.try_fill_tx_env(evm.tx_mut())?;
            evm.transact_commit()?;
            index += 1;
        }
        Ok(index)
    }

    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    fn call_gas_limit(&self) -> u64;

    /// Executes the closure with the state that corresponds to the given [BlockId].
    fn with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        Self: LoadState,
        F: FnOnce(StateProviderBox) -> EthResult<T>,
    {
        let state = self.state_at_block_id(at)?;
        f(state)
    }

    /// Executes the closure with the state that corresponds to the given [BlockId] on a new task
    async fn spawn_with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        Self: LoadState,
        F: FnOnce(StateProviderBox) -> EthResult<T> + Send + 'static,
        T: Send + 'static;

    /// Returns the revm evm env for the requested [BlockId]
    ///
    /// If the [BlockId] this will return the [BlockId] of the block the env was configured
    /// for.
    /// If the [BlockId] is pending, this will return the "Pending" tag, otherwise this returns the
    /// hash of the exact block.
    async fn evm_env_at(&self, at: BlockId) -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv, BlockId)>
    where
        Self: LoadState;

    /// Returns the revm evm env for the raw block header
    ///
    /// This is used for tracing raw blocks
    async fn evm_env_for_raw_block(
        &self,
        header: &Header,
    ) -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv)>
    where
        Self: LoadState,
    {
        // get the parent config first
        let (cfg, mut block_env, _) = self.evm_env_at(header.parent_hash.into()).await?;

        let after_merge = cfg.handler_cfg.spec_id >= SpecId::MERGE;
        fill_block_env_with_coinbase(&mut block_env, header, after_merge, header.beneficiary);

        Ok((cfg, block_env))
    }

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    async fn transactions_by_block(
        &self,
        block: B256,
    ) -> EthResult<Option<Vec<TransactionSigned>>> {
        Ok(self.cache().get_block_transactions(block).await?)
    }

    /// Get the entire block for the given id.
    ///
    /// Returns `None` if block does not exist.
    async fn block_by_id(&self, id: BlockId) -> EthResult<Option<SealedBlock>>;

    /// Get the entire block for the given id.
    ///
    /// Returns `None` if block does not exist.
    async fn block_by_id_with_senders(
        &self,
        id: BlockId,
    ) -> EthResult<Option<SealedBlockWithSenders>>;

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    async fn transactions_by_block_id(
        &self,
        block: BlockId,
    ) -> EthResult<Option<Vec<TransactionSigned>>>;

    /// Returns the EIP-2718 encoded transaction by hash.
    ///
    /// If this is a pooled EIP-4844 transaction, the blob sidecar is included.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    async fn raw_transaction_by_hash(&self, hash: B256) -> EthResult<Option<Bytes>>
    where
        Self: SpawnBlocking,
    {
        // Note: this is mostly used to fetch pooled transactions so we check the pool first
        if let Some(tx) =
            self.pool().get_pooled_transaction_element(hash).map(|tx| tx.envelope_encoded())
        {
            return Ok(Some(tx))
        }

        self.spawn_blocking_io(move |this| {
            Ok(this.provider().transaction_by_hash(hash)?.map(|tx| tx.envelope_encoded()))
        })
        .await
    }

    /// Returns the transaction by hash.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    async fn transaction_by_hash(&self, hash: B256) -> EthResult<Option<TransactionSource>>
    where
        Self: SpawnBlocking,
    {
        // Try to find the transaction on disk
        let mut resp = self
            .spawn_blocking_io(move |this| {
                match this.provider().transaction_by_hash_with_meta(hash)? {
                    None => Ok(None),
                    Some((tx, meta)) => {
                        // Note: we assume this transaction is valid, because it's mined (or part of
                        // pending block) and already. We don't need to
                        // check for pre EIP-2 because this transaction could be pre-EIP-2.
                        let transaction = tx
                            .into_ecrecovered_unchecked()
                            .ok_or(EthApiError::InvalidTransactionSignature)?;

                        let tx = TransactionSource::Block {
                            transaction,
                            index: meta.index,
                            block_hash: meta.block_hash,
                            block_number: meta.block_number,
                            base_fee: meta.base_fee,
                        };
                        Ok(Some(tx))
                    }
                }
            })
            .await?;

        if resp.is_none() {
            // tx not found on disk, check pool
            if let Some(tx) =
                self.pool().get(&hash).map(|tx| tx.transaction.to_recovered_transaction())
            {
                resp = Some(TransactionSource::Pool(tx));
            }
        }

        Ok(resp)
    }

    /// Returns the transaction by including its corresponding [BlockId]
    ///
    /// Note: this supports pending transactions
    async fn transaction_by_hash_at(
        &self,
        transaction_hash: B256,
    ) -> EthResult<Option<(TransactionSource, BlockId)>>
    where
        Self: SpawnBlocking,
    {
        match self.transaction_by_hash(transaction_hash).await? {
            None => return Ok(None),
            Some(tx) => {
                let res = match tx {
                    tx @ TransactionSource::Pool(_) => (tx, BlockId::pending()),
                    TransactionSource::Block {
                        transaction,
                        index,
                        block_hash,
                        block_number,
                        base_fee,
                    } => {
                        let at = BlockId::Hash(block_hash.into());
                        let tx = TransactionSource::Block {
                            transaction,
                            index,
                            block_hash,
                            block_number,
                            base_fee,
                        };
                        (tx, at)
                    }
                };
                Ok(Some(res))
            }
        }
    }

    /// Returns the _historical_ transaction and the block it was mined in
    async fn historical_transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, B256)>>
    where
        Self: SpawnBlocking,
    {
        match self.transaction_by_hash_at(hash).await? {
            None => Ok(None),
            Some((tx, at)) => Ok(at.as_block_hash().map(|hash| (tx, hash))),
        }
    }

    /// Returns the transaction receipt for the given hash.
    ///
    /// Returns None if the transaction does not exist or is pending
    /// Note: The tx receipt is not available for pending transactions.
    async fn transaction_receipt(&self, hash: B256) -> EthResult<Option<AnyTransactionReceipt>>
    where
        Self: BuildReceipt + SpawnBlocking + Clone + 'static,
    {
        let result = self.load_transaction_and_receipt(hash).await?;

        let (tx, meta, receipt) = match result {
            Some((tx, meta, receipt)) => (tx, meta, receipt),
            None => return Ok(None),
        };

        self.build_transaction_receipt(tx, meta, receipt).await.map(Some)
    }

    /// Helper method that loads a transaction and its receipt.
    async fn load_transaction_and_receipt(
        &self,
        hash: TxHash,
    ) -> EthResult<Option<(TransactionSigned, TransactionMeta, Receipt)>>
    where
        Self: SpawnBlocking + Clone + 'static,
    {
        let this = self.clone();
        self.spawn_blocking_io(move |_| {
            let (tx, meta) = match this.provider().transaction_by_hash_with_meta(hash)? {
                Some((tx, meta)) => (tx, meta),
                None => return Ok(None),
            };

            let receipt = match this.provider().receipt_by_hash(hash)? {
                Some(recpt) => recpt,
                None => return Ok(None),
            };

            Ok(Some((tx, meta, receipt)))
        })
        .await
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> EthResult<B256> {
        // On optimism, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        if let Some(client) = self.raw_tx_forwarder().as_ref() {
            tracing::debug!( target: "rpc::eth",  "forwarding raw transaction to");
            client.forward_raw_transaction(&tx).await?;
        }

        let recovered = recover_raw_transaction(tx)?;
        let pool_transaction =
            <Self::Pool as TransactionPool>::Transaction::from_recovered_pooled_transaction(
                recovered,
            );

        // submit the transaction to the pool with a `Local` origin
        let hash = self.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

        Ok(hash)
    }

    /// Signs transaction with a matching signer, if any and submits the transaction to the pool.
    /// Returns the hash of the signed transaction.
    async fn send_transaction(&self, mut request: TransactionRequest) -> EthResult<B256>
    where
        Self: EthApiSpec + LoadState;

    /// Returns the number of transactions sent from an address at the given block identifier.
    ///
    /// If this is [`BlockNumberOrTag::Pending`](reth_primitives::BlockNumberOrTag) then this will
    /// look up the highest transaction in pool and return the next nonce (highest + 1).
    fn get_transaction_count(&self, address: Address, block_id: Option<BlockId>) -> EthResult<U256>
    where
        Self: LoadState,
    {
        if block_id == Some(BlockId::pending()) {
            let address_txs = self.pool().get_transactions_by_sender(address);
            if let Some(highest_nonce) =
                address_txs.iter().map(|item| item.transaction.nonce()).max()
            {
                let tx_count = highest_nonce
                    .checked_add(1)
                    .ok_or(RpcInvalidTransactionError::NonceMaxValue)?;
                return Ok(U256::from(tx_count))
            }
        }

        let state = self.state_at_block_id_or_latest(block_id)?;
        Ok(U256::from(state.account_nonce(address)?.unwrap_or_default()))
    }

    /// Prepares the state and env for the given [TransactionRequest] at the given [BlockId] and
    /// executes the closure on a new task returning the result of the closure.
    ///
    /// This returns the configured [EnvWithHandlerCfg] for the given [TransactionRequest] at the
    /// given [BlockId] and with configured call settings: `prepare_call_env`.
    async fn spawn_with_call_at<F, R>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        f: F,
    ) -> EthResult<R>
    where
        Self: LoadState,
        F: FnOnce(&mut StateCacheDB, EnvWithHandlerCfg) -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Executes the call request at the given [BlockId].
    async fn transact_call_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        Self: LoadState;

    /// Executes the call request at the given [BlockId] on a new task and returns the result of the
    /// inspect call.
    async fn spawn_inspect_call_at<I>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        inspector: I,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        Self: LoadState,
        I: for<'a> Inspector<&'a mut StateCacheDB> + Send + 'static;

    /// Executes the transaction on top of the given [BlockId] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [TracingInspector] and the [ResultAndState] after the
    /// configured [EnvWithHandlerCfg] was inspected.
    ///
    /// Caution: this is blocking
    fn trace_at<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        Self: LoadState,
        F: FnOnce(TracingInspector, ResultAndState) -> EthResult<R>;

    /// Same as [Self::trace_at] but also provides the used database to the callback.
    ///
    /// Executes the transaction on top of the given [BlockId] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [TracingInspector] and the [ResultAndState] after the
    /// configured [EnvWithHandlerCfg] was inspected.
    async fn spawn_trace_at_with_state<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        Self: LoadState,
        F: FnOnce(TracingInspector, ResultAndState, StateCacheDB) -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Fetches the transaction and the transaction's block
    async fn transaction_and_block(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, SealedBlockWithSenders)>>;

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, all previous transaction in the block are applied to the
    /// state by executing them first.
    /// The callback `f` is invoked with the [ResultAndState] after the transaction was executed and
    /// the database that points to the beginning of the transaction.
    ///
    /// Note: Implementers should use a threadpool where blocking is allowed, such as
    /// [BlockingTaskPool](reth_tasks::pool::BlockingTaskPool).
    async fn spawn_trace_transaction_in_block<F, R>(
        &self,
        hash: B256,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<R>>
    where
        Self: LoadState,
        F: FnOnce(TransactionInfo, TracingInspector, ResultAndState, StateCacheDB) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.spawn_trace_transaction_in_block_with_inspector(hash, TracingInspector::new(config), f)
            .await
    }

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, all previous transaction in the block are applied to the
    /// state by executing them first.
    /// The callback `f` is invoked with the [ResultAndState] after the transaction was executed and
    /// the database that points to the beginning of the transaction.
    ///
    /// Note: Implementers should use a threadpool where blocking is allowed, such as
    /// [BlockingTaskPool](reth_tasks::pool::BlockingTaskPool).
    async fn spawn_replay_transaction<F, R>(&self, hash: B256, f: F) -> EthResult<Option<R>>
    where
        Self: LoadState,
        F: FnOnce(TransactionInfo, ResultAndState, StateCacheDB) -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, all previous transaction in the block are applied to the
    /// state by executing them first.
    /// The callback `f` is invoked with the [ResultAndState] after the transaction was executed and
    /// the database that points to the beginning of the transaction.
    ///
    /// Note: Implementers should use a threadpool where blocking is allowed, such as
    /// [BlockingTaskPool](reth_tasks::pool::BlockingTaskPool).
    async fn spawn_trace_transaction_in_block_with_inspector<Insp, F, R>(
        &self,
        hash: B256,
        inspector: Insp,
        f: F,
    ) -> EthResult<Option<R>>
    where
        Self: LoadState,
        F: FnOnce(TransactionInfo, Insp, ResultAndState, StateCacheDB) -> EthResult<R>
            + Send
            + 'static,
        Insp: for<'a> Inspector<&'a mut StateCacheDB> + Send + 'static,
        R: Send + 'static;

    /// Executes all transactions of a block and returns a list of callback results invoked for each
    /// transaction in the block.
    ///
    /// This
    /// 1. fetches all transactions of the block
    /// 2. configures the EVM evn
    /// 3. loops over all transactions and executes them
    /// 4. calls the callback with the transaction info, the execution result, the changed state
    /// _after_ the transaction [StateProviderDatabase] and the database that points to the state
    /// right _before_ the transaction.
    async fn trace_block_with<F, R>(
        &self,
        block_id: BlockId,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        Self: LoadState,
        // This is the callback that's invoked for each transaction with the inspector, the result,
        // state and db
        F: for<'a> Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &'a EvmState,
                &'a StateCacheDB,
            ) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.trace_block_until(block_id, None, config, f).await
    }

    /// Executes all transactions of a block and returns a list of callback results invoked for each
    /// transaction in the block.
    ///
    /// This
    /// 1. fetches all transactions of the block
    /// 2. configures the EVM evn
    /// 3. loops over all transactions and executes them
    /// 4. calls the callback with the transaction info, the execution result, the changed state
    /// _after_ the transaction [EvmState] and the database that points to the state
    /// right _before_ the transaction, in other words the state the transaction was
    /// executed on: `changed_state = tx(cached_state)`
    ///
    /// This accepts a `inspector_setup` closure that returns the inspector to be used for tracing
    /// a transaction. This is invoked for each transaction.
    async fn trace_block_with_inspector<Setup, Insp, F, R>(
        &self,
        block_id: BlockId,
        insp_setup: Setup,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        Self: LoadState,
        // This is the callback that's invoked for each transaction with the inspector, the result,
        // state and db
        F: for<'a> Fn(
                TransactionInfo,
                Insp,
                ExecutionResult,
                &'a EvmState,
                &'a StateCacheDB,
            ) -> EthResult<R>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: for<'a> Inspector<&'a mut StateCacheDB> + Send + 'static,
        R: Send + 'static,
    {
        self.trace_block_until_with_inspector(block_id, None, insp_setup, f).await
    }

    /// Executes all transactions of a block.
    ///
    /// If a `highest_index` is given, this will only execute the first `highest_index`
    /// transactions, in other words, it will stop executing transactions after the
    /// `highest_index`th transaction.
    async fn trace_block_until<F, R>(
        &self,
        block_id: BlockId,
        highest_index: Option<u64>,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        Self: LoadState,
        F: for<'a> Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &'a EvmState,
                &'a StateCacheDB,
            ) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.trace_block_until_with_inspector(
            block_id,
            highest_index,
            move || TracingInspector::new(config),
            f,
        )
        .await
    }

    /// Executes all transactions of a block.
    ///
    /// If a `highest_index` is given, this will only execute the first `highest_index`
    /// transactions, in other words, it will stop executing transactions after the
    /// `highest_index`th transaction.
    ///
    /// Note: This expect tx index to be 0-indexed, so the first transaction is at index 0.
    ///
    /// This accepts a `inspector_setup` closure that returns the inspector to be used for tracing
    /// the transactions.
    async fn trace_block_until_with_inspector<Setup, Insp, F, R>(
        &self,
        block_id: BlockId,
        highest_index: Option<u64>,
        inspector_setup: Setup,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        Self: LoadState,
        F: for<'a> Fn(
                TransactionInfo,
                Insp,
                ExecutionResult,
                &'a EvmState,
                &'a StateCacheDB,
            ) -> EthResult<R>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: for<'a> Inspector<&'a mut StateCacheDB> + Send + 'static,
        R: Send + 'static;
}
