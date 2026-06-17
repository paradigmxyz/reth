//! Loads a pending block from database. Helper trait for `eth_` call and trace RPC methods.

use super::{Call, LoadBlock, LoadState, LoadTransaction};
use crate::{FromEthApiError, FromEvmError};
use alloy_consensus::{
    transaction::{Recovered, TxHashRef},
    BlockHeader,
};
use alloy_eips::BlockId;
use alloy_primitives::B256;
use alloy_rpc_types_eth::TransactionInfo;
use evm2::{
    ethereum::RecoveredTxEnvelope,
    evm::{Db, DbErrorCode},
    registry::HandlerError,
    BaseEvmTypes, TxResultWithState,
};
use evm2_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use futures::Future;
use reth_errors::{ProviderError, RethError};
use reth_evm::{ConfigureEvm, EvmEnvFor, TxEnvFor};
use reth_primitives_traits::{BlockBody, RecoveredBlock};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::{
    ProviderBlock, ProviderTx, SharedEvm2StateProviderDatabase, StateProviderFactory,
};
use std::sync::Arc;

/// Context passed to per-transaction trace callbacks.
pub struct TracingCtx<'a, Insp> {
    /// Execution result and detached state changes for the transaction.
    pub result: TxResultWithState,
    /// Database pointing to the transaction pre-state.
    pub db: &'a mut SharedEvm2StateProviderDatabase,
    /// Inspector used while executing the transaction.
    pub inspector: Insp,
}

impl<Insp> core::fmt::Debug for TracingCtx<'_, Insp> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TracingCtx").finish_non_exhaustive()
    }
}

impl<Insp> TracingCtx<'_, Insp> {
    /// Consumes this context and returns the inspector.
    pub fn take_inspector(self) -> Insp {
        self.inspector
    }
}

/// Executes CPU heavy trace tasks.
pub trait Trace: LoadState<Error: FromEvmError<Self::Evm>> + Call {
    /// Executes a transaction with the provided inspector without committing state changes.
    fn inspect_with_inspector<I>(
        &self,
        db: SharedEvm2StateProviderDatabase,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: TxEnvFor<Self::Evm>,
        inspector: I,
    ) -> Result<(I, TxResultWithState), Self::Error>
    where
        TxEnvFor<Self::Evm>: AsRef<RecoveredTxEnvelope>,
        I: evm2::Inspector<BaseEvmTypes> + 'static,
    {
        let mut evm = self.evm_config().evm_with_env_and_inspector(Db::new(db), evm_env, inspector);

        enum Resolution {
            Result(TxResultWithState),
            DatabaseError(DbErrorCode),
            HandlerError(HandlerError),
        }

        let resolution = match evm.transact(tx_env.as_ref()) {
            Ok(executed) => {
                if let Some(code) = executed.result().db_error_code {
                    let _ = executed.discard();
                    Resolution::DatabaseError(code)
                } else {
                    Resolution::Result(executed.detach())
                }
            }
            Err(err) => Resolution::HandlerError(err),
        };

        let result = match resolution {
            Resolution::Result(result) => result,
            Resolution::DatabaseError(code) => return Err(evm2_db_error(&mut evm, code)),
            Resolution::HandlerError(err) => return Err(evm2_handler_error(&mut evm, err)),
        };

        let inspector =
            evm.clear_inspector_as::<I>().map(|inspector| *inspector).ok_or_else(|| {
                Self::Error::from_eth_err(EthApiError::EvmCustom(
                    "evm2 inspector missing after trace".to_string(),
                ))
            })?;

        Ok((inspector, result))
    }

    /// Executes a closure against a shared evm2 cache initialized from the requested block state.
    fn spawn_with_evm2_state_at_block<F, R>(
        &self,
        at: BlockId,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(Self, SharedEvm2StateProviderDatabase) -> Result<R, Self::Error> + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_tracing(move |this| {
            let state = this.provider().state_by_block_id(at).map_err(Self::Error::from_eth_err)?;
            // SAFETY: the shared database is used only within this synchronous closure, and the
            // boxed state provider is held until the closure returns.
            let db = unsafe { SharedEvm2StateProviderDatabase::new(&*state) };
            f(this, db)
        })
    }

    /// Retrieves and traces a transaction in its containing block.
    fn spawn_trace_transaction_in_block<F, R>(
        &self,
        hash: B256,
        config: TracingInspectorConfig,
        f: F,
    ) -> impl Future<Output = Result<Option<R>, Self::Error>> + Send
    where
        Self: LoadBlock + LoadTransaction,
        F: FnOnce(
                TransactionInfo,
                TracingInspector,
                TxResultWithState,
                SharedEvm2StateProviderDatabase,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
        TxEnvFor<Self::Evm>: AsRef<RecoveredTxEnvelope>,
        ProviderTx<Self::Provider>: Clone,
    {
        self.spawn_trace_transaction_in_block_with_inspector(hash, TracingInspector::new(config), f)
    }

    /// Retrieves and traces a transaction in its containing block with a custom inspector.
    fn spawn_trace_transaction_in_block_with_inspector<Insp, F, R>(
        &self,
        hash: B256,
        inspector: Insp,
        f: F,
    ) -> impl Future<Output = Result<Option<R>, Self::Error>> + Send
    where
        Self: LoadBlock + LoadTransaction,
        F: FnOnce(
                TransactionInfo,
                Insp,
                TxResultWithState,
                SharedEvm2StateProviderDatabase,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Insp: evm2::Inspector<BaseEvmTypes> + Send + 'static,
        R: Send + 'static,
        TxEnvFor<Self::Evm>: AsRef<RecoveredTxEnvelope>,
        ProviderTx<Self::Provider>: Clone,
    {
        async move {
            let (transaction, block) = match self.transaction_and_block(hash).await? {
                None => return Ok(None),
                Some(res) => res,
            };
            let (target_tx, tx_info) = transaction.split();
            let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

            self.spawn_with_evm2_state_at_block(block.parent_hash().into(), move |this, db| {
                this.apply_pre_execution_changes(&block, evm_env.clone(), db.clone())?;

                this.replay_transactions_until(
                    db.clone(),
                    evm_env.clone(),
                    block.transactions_recovered(),
                    *target_tx.tx_hash(),
                )?;

                let tx_env = TxEnvFor::<Self::Evm>::from(target_tx.clone());
                let (inspector, result) =
                    this.inspect_with_inspector(db.clone(), evm_env, tx_env, inspector)?;
                f(tx_info, inspector, result, db)
            })
            .await
            .map(Some)
        }
    }

    /// Executes transactions until the target transaction hash, excluding the target itself.
    fn replay_transactions_until<'a, I>(
        &self,
        db: SharedEvm2StateProviderDatabase,
        evm_env: EvmEnvFor<Self::Evm>,
        transactions: I,
        target_tx_hash: B256,
    ) -> Result<u64, Self::Error>
    where
        I: IntoIterator<Item = Recovered<&'a ProviderTx<Self::Provider>>>,
        TxEnvFor<Self::Evm>: AsRef<RecoveredTxEnvelope>,
        ProviderTx<Self::Provider>: Clone + 'a,
    {
        for (idx, tx) in transactions.into_iter().enumerate() {
            if *tx.tx_hash() == target_tx_hash {
                return Ok(idx as u64)
            }

            let tx_env = TxEnvFor::<Self::Evm>::from(tx.cloned());
            let (_, result) = self.inspect_with_inspector(
                db.clone(),
                evm_env.clone(),
                tx_env,
                evm2::NoopInspector::default(),
            )?;
            db.commit_source(&result.state_changes).map_err(Self::Error::from_eth_err)?;
        }

        Err(EthApiError::TransactionNotFound.into())
    }

    /// Executes all transactions of a block up to a given index.
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
        F: Fn(TransactionInfo, TracingCtx<'_, Insp>) -> Result<R, Self::Error> + Send + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: evm2::Inspector<BaseEvmTypes> + Send + 'static,
        R: Send + 'static,
        TxEnvFor<Self::Evm>: AsRef<RecoveredTxEnvelope>,
        ProviderTx<Self::Provider>: Clone,
    {
        async move {
            let block =
                if block.is_some() { block } else { self.recovered_block(block_id).await? };
            let Some(block) = block else { return Ok(None) };
            let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

            if block.body().transactions().is_empty() {
                return Ok(Some(Vec::new()))
            }

            self.spawn_with_evm2_state_at_block(block.parent_hash().into(), move |this, mut db| {
                let block_hash = block.hash();
                let block_number = block.number();
                let block_timestamp = block.timestamp();
                let base_fee = block.base_fee_per_gas();

                this.apply_pre_execution_changes(&block, evm_env.clone(), db.clone())?;

                let max_transactions = highest_index
                    .map(|highest| highest as usize + 1)
                    .unwrap_or_else(|| block.body().transaction_count());
                let mut results = Vec::new();

                for (idx, tx) in block.transactions_recovered().take(max_transactions).enumerate() {
                    let tx_env = TxEnvFor::<Self::Evm>::from(tx.cloned());
                    let (inspector, result) = this.inspect_with_inspector(
                        db.clone(),
                        evm_env.clone(),
                        tx_env,
                        inspector_setup(),
                    )?;
                    let tx_info = TransactionInfo {
                        hash: Some(*tx.tx_hash()),
                        index: Some(idx as u64),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        block_timestamp: Some(block_timestamp),
                        base_fee,
                    };
                    let state_changes = result.state_changes.clone();
                    let item = f(tx_info, TracingCtx { result, db: &mut db, inspector })?;
                    db.commit_source(&state_changes).map_err(Self::Error::from_eth_err)?;
                    results.push(item);
                }

                Ok(Some(results))
            })
            .await
        }
    }

    /// Executes all transactions of a block with a tracing inspector.
    fn trace_block_with<F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>>,
        config: TracingInspectorConfig,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        F: Fn(TransactionInfo, TracingCtx<'_, TracingInspector>) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
        TxEnvFor<Self::Evm>: AsRef<RecoveredTxEnvelope>,
        ProviderTx<Self::Provider>: Clone,
    {
        self.trace_block_until_with_inspector(
            block_id,
            block,
            None,
            move || TracingInspector::new(config),
            f,
        )
    }

    /// Executes all transactions of a block with a custom inspector.
    fn trace_block_inspector<Setup, Insp, F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>>,
        insp_setup: Setup,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        F: Fn(TransactionInfo, TracingCtx<'_, Insp>) -> Result<R, Self::Error> + Send + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: evm2::Inspector<BaseEvmTypes> + Send + 'static,
        R: Send + 'static,
        TxEnvFor<Self::Evm>: AsRef<RecoveredTxEnvelope>,
        ProviderTx<Self::Provider>: Clone,
    {
        self.trace_block_until_with_inspector(block_id, block, None, insp_setup, f)
    }

    /// Applies chain-specific state transitions required before executing a block.
    fn apply_pre_execution_changes(
        &self,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        evm_env: EvmEnvFor<Self::Evm>,
        db: SharedEvm2StateProviderDatabase,
    ) -> Result<(), Self::Error> {
        let ctx = self
            .evm_config()
            .context_for_block(block.sealed_block())
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;
        let changes = self
            .evm_config()
            .pre_block_state_changes(db.clone(), evm_env, block.number(), ctx)
            .map_err(|err| EthApiError::EvmCustom(err.to_string()))
            .map_err(Self::Error::from_eth_err)?;
        db.commit_source(&changes).map_err(Self::Error::from_eth_err)
    }
}

fn evm2_handler_error<Evm, Error>(evm: &mut evm2::Evm<BaseEvmTypes>, err: HandlerError) -> Error
where
    Error: FromEvmError<Evm>,
{
    match err {
        HandlerError::Database(code) => evm2_db_error::<Evm, Error>(evm, code),
        err => Error::from_eth_err(EthApiError::EvmCustom(err.to_string())),
    }
}

fn evm2_db_error<Evm, Error>(evm: &mut evm2::Evm<BaseEvmTypes>, code: DbErrorCode) -> Error
where
    Error: FromEvmError<Evm>,
{
    let err = evm.database_mut().error(code);
    match err.downcast::<ProviderError>() {
        Ok(err) => Error::from_eth_err(*err),
        Err(err) => Error::from_eth_err(EthApiError::EvmCustom(format!(
            "evm2 database error {code:?}: {err}"
        ))),
    }
}

#[cfg(any())]
pub trait Trace: LoadState<Error: FromEvmError<Self::Evm>> + Call {
    /// Executes the [`TxEnvFor`] with [`reth_evm::EvmEnv`] against the given [Database] without
    /// committing state changes.
    fn inspect<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: TxEnvFor<Self::Evm>,
        inspector: I,
    ) -> Result<ResultAndState<HaltReasonFor<Self::Evm>>, Self::Error>
    where
        DB: Database<Error = EvmDatabaseError<ProviderError>>,
        I: InspectorFor<Self::Evm, DB>,
    {
        let mut evm = self.evm_config().evm_with_env_and_inspector(db, evm_env, inspector);
        evm.transact(tx_env).map_err(Self::Error::from_evm_err)
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
                ResultAndState<HaltReasonFor<Self::Evm>>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
    {
        self.with_state_at_block(at, move |this, state| {
            let mut db = State::builder().with_database(StateProviderDatabase::new(state)).build();
            let mut inspector = TracingInspector::new(config);
            let res = this.inspect(&mut db, evm_env, tx_env, &mut inspector)?;
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
                ResultAndState<HaltReasonFor<Self::Evm>>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.spawn_with_state_at_block(at, move |this, mut db| {
            let mut inspector = TracingInspector::new(config);
            let res = this.inspect(&mut db, evm_env, tx_env, &mut inspector)?;
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
                ResultAndState<HaltReasonFor<Self::Evm>>,
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
        mut inspector: Insp,
        f: F,
    ) -> impl Future<Output = Result<Option<R>, Self::Error>> + Send
    where
        Self: LoadTransaction,
        F: FnOnce(
                TransactionInfo,
                Insp,
                ResultAndState<HaltReasonFor<Self::Evm>>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Insp: for<'a> InspectorFor<Self::Evm, &'a mut StateCacheDb> + Send + 'static,
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
                let res = this.inspect(&mut db, evm_env, tx_env, &mut inspector)?;
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
                TracingCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    EvmFor<Self::Evm, &mut StateCacheDb, TracingInspector>,
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
                TracingCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    EvmFor<Self::Evm, &mut StateCacheDb, Insp>,
                >,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: Clone + for<'a> InspectorFor<Self::Evm, &'a mut StateCacheDb>,
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

                let mut idx = 0;

                let results = this
                    .evm_config()
                    .evm_factory()
                    .create_tracer(&mut db, evm_env, inspector_setup())
                    .try_trace_many(block.transactions_recovered().take(max_transactions), |ctx| {
                        let tx_info = TransactionInfo {
                            hash: Some(*ctx.tx.tx_hash()),
                            index: Some(idx),
                            block_hash: Some(block_hash),
                            block_number: Some(block_number),
                            block_timestamp: Some(block_timestamp),
                            base_fee: Some(base_fee),
                        };
                        idx += 1;

                        f(tx_info, ctx)
                    })
                    .collect::<Result<_, _>>()?;

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
                TracingCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    EvmFor<Self::Evm, &mut StateCacheDb, TracingInspector>,
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
                TracingCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    EvmFor<Self::Evm, &mut StateCacheDb, Insp>,
                >,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: Clone + for<'a> InspectorFor<Self::Evm, &'a mut StateCacheDb>,
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
            .map_err(reth_errors::BlockExecutionError::other)
            .map_err(Self::Error::from_eth_err)?;
        Ok(())
    }
}
