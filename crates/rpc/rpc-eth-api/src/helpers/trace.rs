//! Loads a pending block from database. Helper trait for `eth_` call and trace RPC methods.

use super::{Call, LoadBlock, LoadState, LoadTransaction};
use crate::{FromEthApiError, FromEvmError};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_primitives::B256;
use alloy_rpc_types_eth::{BlockId, TransactionInfo};
use evm2_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use futures::Future;
use reth_errors::{ProviderError, RethError};
use reth_evm::{
    context::{Block, ExecutionResult, ResultAndState},
    database::{Database, DatabaseCommit, EvmDatabaseError, State, StateProviderDatabase},
    evm2::{
        block_env_from_revm, create_evm2_db_ref, ethereum_tx_env_from_revm, inspect_tx_env_for,
        Evm2InspectResult,
    },
    execute::BlockExecutor,
    ConfigureEvm, EvmEnvFor, HaltReasonFor, TxEnvFor,
};
use reth_primitives_traits::{BlockBody, Recovered, RecoveredBlock};
use reth_rpc_eth_types::cache::db::StateCacheDb;
use reth_storage_api::{ProviderBlock, ProviderTx};
use std::sync::Arc;

/// Context exposed while tracing a block transaction.
#[derive(Debug)]
pub struct BlockTraceCtx<'a, T, I, H> {
    /// The transaction that was just executed.
    pub tx: T,
    /// Result of transaction execution.
    pub result: ExecutionResult<H>,
    /// Native evm2 inspection result.
    pub evm2: Evm2InspectResult<H>,
    /// Inspector state after transaction.
    pub inspector: I,
    /// Database used when executing the transaction, before committing state changes.
    pub db: &'a mut StateCacheDb,
    fused_inspector: I,
}

impl<T, I, H> BlockTraceCtx<'_, T, I, H>
where
    I: Clone,
{
    /// Fuses the inspector and returns the current inspector state.
    pub fn take_inspector(&mut self) -> I {
        core::mem::replace(&mut self.inspector, self.fused_inspector.clone())
    }

    /// Returns an evm2 database adapter over the pre-transaction database.
    pub fn evm2_db(&mut self) -> evm2::evm::Db<reth_evm::evm2::Evm2DatabaseRef> {
        create_evm2_db_ref(self.db)
    }
}

/// Executes CPU heavy tasks.
pub trait Trace: LoadState<Error: FromEvmError<Self::Evm>> + Call {
    /// Executes the [`TxEnvFor`] with [`reth_evm::EvmEnv`] against the given [Database] without
    /// committing state changes.
    fn inspect<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: TxEnvFor<Self::Evm>,
        inspector: I,
    ) -> Result<(I, Evm2InspectResult<HaltReasonFor<Self::Evm>>), Self::Error>
    where
        DB: Database<Error = EvmDatabaseError<ProviderError>> + core::fmt::Debug,
        I: evm2::Inspector<evm2::BaseEvmTypes> + 'static,
    {
        let mut db = db;
        let block_env = block_env_from_revm(evm_env.block_env);
        let tx_env = ethereum_tx_env_from_revm(&tx_env);
        inspect_tx_env_for::<Self::Evm, _, _>(
            &mut db,
            *evm_env.cfg_env.spec(),
            block_env,
            tx_env,
            inspector,
        )
        .map_err(RethError::other)
        .map_err(Self::Error::from_eth_err)
    }

    /// Executes the transaction on top of the given [`BlockId`] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [`TracingInspector`] and the [`ResultAndState`] after
    /// the configured [`reth_evm::EvmEnv`] was inspected.
    ///
    /// Caution: this is blocking
    fn trace_at<F, R>(
        &self,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: TxEnvFor<Self::Evm>,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        R: Send + 'static,
        F: FnOnce(
                TracingInspector,
                Evm2InspectResult<HaltReasonFor<Self::Evm>>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
    {
        self.with_state_at_block(at, move |this, state| {
            let mut db = State::builder().with_database(StateProviderDatabase::new(state)).build();
            let inspector = TracingInspector::new(config);
            let (inspector, res) = this.inspect(&mut db, evm_env, tx_env, inspector)?;
            f(inspector, res)
        })
    }

    /// Same as [`trace_at`](Self::trace_at) but also provides the used database to the callback.
    ///
    /// Executes the transaction on top of the given [`BlockId`] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [`TracingInspector`] and the [`ResultAndState`] after
    /// the configured [`reth_evm::EvmEnv`] was inspected.
    fn spawn_trace_at_with_state<F, R>(
        &self,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: TxEnvFor<Self::Evm>,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(
                TracingInspector,
                Evm2InspectResult<HaltReasonFor<Self::Evm>>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.spawn_with_state_at_block(at, move |this, mut db| {
            let inspector = TracingInspector::new(config);
            let (inspector, res) = this.inspect(&mut db, evm_env, tx_env, inspector)?;
            f(inspector, res, db)
        })
    }

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, all previous transaction in the block are applied to the
    /// state by executing them first.
    /// The callback `f` is invoked with the [`ResultAndState`] after the transaction was executed
    /// and the database that points to the beginning of the transaction.
    ///
    /// Note: Implementers should use a threadpool where blocking is allowed, such as
    /// [`BlockingTaskPool`](reth_tasks::pool::BlockingTaskPool).
    fn spawn_trace_transaction_in_block<F, R>(
        &self,
        hash: B256,
        config: TracingInspectorConfig,
        f: F,
    ) -> impl Future<Output = Result<Option<R>, Self::Error>> + Send
    where
        Self: LoadTransaction,
        F: FnOnce(
                TransactionInfo,
                TracingInspector,
                Evm2InspectResult<HaltReasonFor<Self::Evm>>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.spawn_trace_transaction_in_block_with_inspector(hash, TracingInspector::new(config), f)
    }

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, all previous transaction in the block are applied to the
    /// state by executing them first.
    /// The callback `f` is invoked with the [`ResultAndState`] after the transaction was executed
    /// and the database that points to the beginning of the transaction.
    ///
    /// Note: Implementers should use a threadpool where blocking is allowed, such as
    /// [`BlockingTaskPool`](reth_tasks::pool::BlockingTaskPool).
    fn spawn_trace_transaction_in_block_with_inspector<Insp, F, R>(
        &self,
        hash: B256,
        inspector: Insp,
        f: F,
    ) -> impl Future<Output = Result<Option<R>, Self::Error>> + Send
    where
        Self: LoadTransaction,
        F: FnOnce(
                TransactionInfo,
                Insp,
                Evm2InspectResult<HaltReasonFor<Self::Evm>>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Insp: evm2::Inspector<evm2::BaseEvmTypes> + Send + 'static,
        R: Send + 'static,
    {
        async move {
            let (transaction, block) = match self.transaction_and_block(hash).await? {
                None => return Ok(None),
                Some(res) => res,
            };
            let (tx, tx_info) = transaction.split();

            let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

            // we need to get the state of the parent block because we're essentially replaying the
            // block the transaction is included in
            let parent_block = block.parent_hash();

            self.spawn_with_state_at_block(parent_block, move |this, mut db| {
                let block_txs = block.transactions_recovered();

                this.apply_pre_execution_changes(&block, &mut db)?;

                // replay all transactions prior to the targeted transaction
                this.replay_transactions_until(&mut db, evm_env.clone(), block_txs, *tx.tx_hash())?;

                let tx_env = this.evm_config().tx_env(tx);
                let (inspector, res) = this.inspect(&mut db, evm_env, tx_env, inspector)?;
                f(tx_info, inspector, res, db)
            })
            .await
            .map(Some)
        }
    }

    /// Executes all transactions of a block up to a given index.
    ///
    /// If a `highest_index` is given, this will only execute the first `highest_index`
    /// transactions, in other words, it will stop executing transactions after the
    /// `highest_index`th transaction. If `highest_index` is `None`, all transactions
    /// are executed.
    fn trace_block_until<F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>>,
        highest_index: Option<u64>,
        config: TracingInspectorConfig,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        F: Fn(
                TransactionInfo,
                BlockTraceCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    TracingInspector,
                    HaltReasonFor<Self::Evm>,
                >,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.trace_block_until_with_inspector(
            block_id,
            block,
            highest_index,
            move || TracingInspector::new(config),
            f,
        )
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
    fn trace_block_until_with_inspector<Setup, Insp, F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>>,
        highest_index: Option<u64>,
        mut inspector_setup: Setup,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        F: Fn(
                TransactionInfo,
                BlockTraceCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    Insp,
                    HaltReasonFor<Self::Evm>,
                >,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: Clone + evm2::Inspector<evm2::BaseEvmTypes> + Send + 'static,
        R: Send + 'static,
    {
        async move {
            let block =
                if block.is_some() { block } else { self.recovered_block(block_id).await? };

            let Some(block) = block else { return Ok(None) };
            let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

            if block.body().transactions().is_empty() {
                // nothing to trace
                return Ok(Some(Vec::new()))
            }

            // replay all transactions of the block
            // we need to get the state of the parent block because we're replaying this block
            // on top of its parent block's state
            self.spawn_with_state_at_block(block.parent_hash(), move |this, mut db| {
                let block_hash = block.hash();

                let block_number = evm_env.block_env.number().saturating_to();
                let block_timestamp = evm_env.block_env.timestamp().saturating_to();
                let base_fee = evm_env.block_env.basefee();

                this.apply_pre_execution_changes(&block, &mut db)?;

                // prepare transactions, we do everything upfront to reduce time spent with open
                // state
                let max_transactions = highest_index.map_or_else(
                    || block.body().transaction_count(),
                    |highest| {
                        // we need + 1 because the index is 0-based
                        highest as usize + 1
                    },
                );

                let mut results = Vec::with_capacity(max_transactions);
                let mut transactions =
                    block.transactions_recovered().take(max_transactions).peekable();
                let mut idx = 0;
                while let Some(tx) = transactions.next() {
                    let tx_env = this.evm_config().tx_env(tx);
                    let fused_inspector = inspector_setup();
                    let (inspector, res) =
                        this.inspect(&mut db, evm_env.clone(), tx_env, fused_inspector.clone())?;
                    let ResultAndState { result, state } = res.result.clone();
                    let ctx = BlockTraceCtx {
                        tx,
                        result,
                        evm2: res,
                        inspector,
                        db: &mut db,
                        fused_inspector,
                    };
                    let tx_info = TransactionInfo {
                        hash: Some(*ctx.tx.tx_hash()),
                        index: Some(idx),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        block_timestamp: Some(block_timestamp),
                        base_fee: Some(base_fee),
                    };
                    idx += 1;

                    results.push(f(tx_info, ctx)?);

                    if transactions.peek().is_some() {
                        db.commit(state);
                    }
                }

                Ok(Some(results))
            })
            .await
        }
    }

    /// Executes all transactions of a block and returns a list of callback results invoked for each
    /// transaction in the block.
    ///
    /// This
    /// 1. fetches all transactions of the block
    /// 2. configures the EVM env
    /// 3. loops over all transactions and executes them
    /// 4. calls the callback with the transaction info, the execution result, the changed state
    ///    _after_ the transaction [`StateProviderDatabase`] and the database that points to the
    ///    state right _before_ the transaction.
    fn trace_block_with<F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>>,
        config: TracingInspectorConfig,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        // This is the callback that's invoked for each transaction with the inspector, the result,
        // state and db
        F: Fn(
                TransactionInfo,
                BlockTraceCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    TracingInspector,
                    HaltReasonFor<Self::Evm>,
                >,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.trace_block_until(block_id, block, None, config, f)
    }

    /// Executes all transactions of a block and returns a list of callback results invoked for each
    /// transaction in the block.
    ///
    /// This
    /// 1. fetches all transactions of the block
    /// 2. configures the EVM env
    /// 3. loops over all transactions and executes them
    /// 4. calls the callback with the transaction info, the execution result, the changed state
    ///    _after_ the transaction `EvmState` and the database that points to the state right
    ///    _before_ the transaction, in other words the state the transaction was executed on:
    ///    `changed_state = tx(cached_state)`
    ///
    /// This accepts a `inspector_setup` closure that returns the inspector to be used for tracing
    /// a transaction. This is invoked for each transaction.
    fn trace_block_inspector<Setup, Insp, F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>>,
        insp_setup: Setup,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        // This is the callback that's invoked for each transaction with the inspector, the result,
        // state and db
        F: Fn(
                TransactionInfo,
                BlockTraceCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    Insp,
                    HaltReasonFor<Self::Evm>,
                >,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: Clone + evm2::Inspector<evm2::BaseEvmTypes> + Send + 'static,
        R: Send + 'static,
    {
        self.trace_block_until_with_inspector(block_id, block, None, insp_setup, f)
    }

    /// Applies chain-specific state transitions required before executing a block.
    ///
    /// Note: This should only be called when tracing an entire block vs individual transactions.
    /// When tracing transactions on top of an already committed block state, those transitions are
    /// already applied.
    fn apply_pre_execution_changes(
        &self,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        db: &mut StateCacheDb,
    ) -> Result<(), Self::Error> {
        self.evm_config()
            .executor_for_block(db, block.sealed_block())
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?
            .apply_pre_execution_changes()
            .map_err(Self::Error::from_eth_err)?;
        Ok(())
    }
}
