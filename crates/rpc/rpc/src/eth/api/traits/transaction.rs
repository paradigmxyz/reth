//! Database access for `eth_` transaction RPC methods. Loads transaction and receipt data w.r.t.
//! network.

use crate::eth::{
    error::{EthApiError, EthResult},
    revm_utils::EvmOverrides,
    TransactionSource,
};
use reth_primitives::{
    BlockId, Bytes, Header, Receipt, SealedBlock, SealedBlockWithSenders, TransactionMeta,
    TransactionSigned, TxHash, B256,
};
use reth_provider::{BlockReaderIdExt, ReceiptProvider, StateProviderBox, TransactionsProvider};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_types::{AnyTransactionReceipt, TransactionInfo, TransactionRequest};
use revm::{
    db::CacheDB,
    primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, EvmState, ExecutionResult,
        ResultAndState,
    },
    GetInspector, Inspector,
};
use revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};

use crate::eth::{api::BuildReceipt, revm_utils::FillableTransaction};
use revm_primitives::db::{Database, DatabaseRef};

use super::SpawnBlocking;

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
    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> &impl BlockReaderIdExt;

    /// Executes the [EnvWithHandlerCfg] against the given [Database] without committing state
    /// changes.
    fn transact<DB>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        DB: Database,
        <DB as Database>::Error: Into<EthApiError>;

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
        I: GetInspector<DB>;

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
        I: GetInspector<DB>;

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
        Tx: FillableTransaction;

    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    fn call_gas_limit(&self) -> u64;

    /// Executes the future on a new blocking task.
    ///
    /// Note: This is expected for futures that are dominated by blocking IO operations, for tracing
    /// or CPU bound operations in general use [Self::spawn_blocking].
    async fn spawn_blocking_future<F, R>(&self, c: F) -> EthResult<R>
    where
        F: Future<Output = EthResult<R>> + Send + 'static,
        R: Send + 'static;

    /// Executes a blocking on the tracing pol.
    ///
    /// Note: This is expected for futures that are predominantly CPU bound, for blocking IO futures
    /// use [Self::spawn_blocking_future].
    async fn spawn_blocking<F, R>(&self, c: F) -> EthResult<R>
    where
        F: FnOnce() -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Returns the state at the given [BlockId]
    fn state_at(&self, at: BlockId) -> EthResult<StateProviderBox>;

    /// Executes the closure with the state that corresponds to the given [BlockId].
    fn with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox) -> EthResult<T>;

    /// Executes the closure with the state that corresponds to the given [BlockId] on a new task
    async fn spawn_with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox) -> EthResult<T> + Send + 'static,
        T: Send + 'static;

    /// Returns the revm evm env for the requested [BlockId]
    ///
    /// If the [BlockId] this will return the [BlockId] of the block the env was configured
    /// for.
    /// If the [BlockId] is pending, this will return the "Pending" tag, otherwise this returns the
    /// hash of the exact block.
    async fn evm_env_at(&self, at: BlockId)
        -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv, BlockId)>;

    /// Returns the revm evm env for the raw block header
    ///
    /// This is used for tracing raw blocks
    async fn evm_env_for_raw_block(
        &self,
        at: &Header,
    ) -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv)>;

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    async fn transactions_by_block(&self, block: B256)
        -> EthResult<Option<Vec<TransactionSigned>>>;

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
    async fn raw_transaction_by_hash(&self, hash: B256) -> EthResult<Option<Bytes>>;

    /// Returns the transaction by hash.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    async fn transaction_by_hash(&self, hash: B256) -> EthResult<Option<TransactionSource>>;

    /// Returns the transaction by including its corresponding [BlockId]
    ///
    /// Note: this supports pending transactions
    async fn transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, BlockId)>>;

    /// Returns the _historical_ transaction and the block it was mined in
    async fn historical_transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, B256)>>;

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
        self.spawn_blocking(move |_| {
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
    async fn send_raw_transaction(&self, tx: Bytes) -> EthResult<B256>;

    /// Signs transaction with a matching signer, if any and submits the transaction to the pool.
    /// Returns the hash of the signed transaction.
    async fn send_transaction(&self, request: TransactionRequest) -> EthResult<B256>;

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
        F: FnOnce(&mut StateCacheDB, EnvWithHandlerCfg) -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Executes the call request at the given [BlockId].
    async fn transact_call_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>;

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
