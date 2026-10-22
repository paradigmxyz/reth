//! Loads a pending block from database. Helper trait for `eth_` call and trace RPC methods.

use super::{Call, LoadBlock, LoadState, LoadTransaction};
use crate::{FromEthApiError, FromEvmError};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::B256;
use alloy_rpc_types_eth::{BlockId, TransactionInfo};
use futures::Future;
use reth_errors::RethError;
use reth_evm::{
    block::{BlockExecutor, BlockExecutorFor, TxResult},
    ConfigureEvm, Evm, EvmEnvFor, EvmFor, HaltReasonFor, InspectorFor, IntoTxEnv, TxEnvFor,
};
use reth_primitives_traits::{BlockBody, BlockTy, Recovered, RecoveredBlock};
use reth_rpc_eth_types::{
    cache::db::{attach_bal_before_tx, StateCacheDb},
    EthApiError,
};
use reth_storage_api::{ProviderBlock, ProviderTx};
use reth_tasks::cancel::is_cancelled;
use revm::{
    context::Block,
    context_interface::result::{ExecutionResult, ResultAndState},
    state::{bal::Bal as RevmBal, EvmState},
};
use revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use std::sync::Arc;

/// Executes CPU heavy tasks.
pub trait Trace: LoadState<Error: FromEvmError<Self::Evm>> + Call {
    /// Executes the [`TxEnvFor`] with [`reth_evm::EvmEnv`] against the given [`StateCacheDb`]
    /// without committing state changes.
    fn inspect<'a>(
        &self,
        db: &'a mut StateCacheDb,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: impl IntoTxEnv<TxEnvFor<Self::Evm>>,
        inspector: impl InspectorFor<Self::Evm, &'a mut StateCacheDb>,
    ) -> Result<ResultAndState<HaltReasonFor<Self::Evm>>, Self::Error> {
        self.evm_config()
            .evm_with_env_and_inspector(db, evm_env, inspector)
            .transact(tx_env)
            .map_err(Self::Error::from_evm_err)
    }

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, the state is positioned right before the transaction,
    /// either by attaching the block's cached BAL or by executing all previous transactions in
    /// the block, see [`Trace::inspect_transaction_in_block`].
    /// The callback `f` is invoked with the [`ResultAndState`] after the transaction was executed
    /// and the database that points to the beginning of the transaction. The database may have
    /// the block's BAL attached and must only be used for reads, because an attached BAL takes
    /// read precedence over state committed on top, see [`attach_bal_before_tx`].
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
    /// Before the transaction is traced, the state is positioned right before the transaction,
    /// either by attaching the block's cached BAL or by executing all previous transactions in
    /// the block, see [`Trace::inspect_transaction_in_block`].
    /// The callback `f` is invoked with the [`ResultAndState`] after the transaction was executed
    /// and the database that points to the beginning of the transaction. The database may have
    /// the block's BAL attached and must only be used for reads, because an attached BAL takes
    /// read precedence over state committed on top, see [`attach_bal_before_tx`].
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
            let (transaction, block, bal) =
                match self.transaction_and_block_and_maybe_bal(hash).await? {
                    None => return Ok(None),
                    Some(res) => res,
                };
            let (tx, tx_info) = transaction.split();

            // we need to get the state of the parent block because we're essentially replaying the
            // block the transaction is included in
            let parent_block = block.parent_hash();

            self.spawn_with_state_at_block(parent_block, move |this, mut db| {
                let (res, _) = this.inspect_transaction_in_block(
                    &block,
                    &mut db,
                    &mut inspector,
                    // index should always be available because `transaction_and_block` only
                    // returns transactions included in a block
                    tx_info.index.expect("transaction_and_block only returns block transactions")
                        as usize,
                    tx,
                    bal.as_deref(),
                )?;
                f(tx_info, inspector, res, db)
            })
            .await
            .map(Some)
        }
    }

    /// Positions the state of `db` right before the transaction at the target index.
    ///
    /// If the block's cached BAL is given, it is attached to the database at the target index and
    /// no transactions are executed, see [`attach_bal_before_tx`]. Otherwise the block's
    /// pre-execution changes and all transactions before the target transaction are executed
    /// through the block executor, and their changes are written to the _runtime_ db
    /// ([`StateCacheDb`]).
    ///
    /// If the target index is greater than or equal to the block's transaction count, all
    /// transactions are replayed.
    fn replay_block_until(
        &self,
        db: &mut StateCacheDb,
        block: &RecoveredBlock<BlockTy<Self::Primitives>>,
        target_tx_index: usize,
        bal: Option<&DecodedBal<Arc<RevmBal>>>,
    ) -> Result<(), Self::Error> {
        if let Some(bal) = bal {
            attach_bal_before_tx(db, bal, target_tx_index);
            return Ok(())
        }

        let mut executor = self
            .evm_config()
            .executor_for_block(db, block.sealed_block())
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;
        executor.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;

        for tx in block.transactions_recovered().take(target_tx_index) {
            executor.execute_transaction(tx).map_err(Self::Error::from_eth_err)?;
        }
        Ok(())
    }

    /// Executes the target transaction with the configured inspector on the state right before
    /// the transaction.
    ///
    /// If the block's cached BAL is given, the state is positioned by attaching the BAL at the
    /// target index, see [`attach_bal_before_tx`], and `target_tx_env` is executed on its own, so
    /// per-transaction changes the block executor applies are skipped. Otherwise all transactions
    /// before the target transaction are replayed without inspection through the block
    /// executor, which then executes the block's transaction at the target index, so the result
    /// includes any per-transaction changes the executor applies. The result is not committed
    /// to `db`.
    #[expect(clippy::type_complexity)]
    fn inspect_transaction_in_block<'a>(
        &self,
        block: &RecoveredBlock<BlockTy<Self::Primitives>>,
        db: &'a mut StateCacheDb,
        inspector: impl InspectorFor<Self::Evm, &'a mut StateCacheDb>,
        target_tx_index: usize,
        target_tx_env: impl IntoTxEnv<TxEnvFor<Self::Evm>>,
        bal: Option<&DecodedBal<Arc<RevmBal>>>,
    ) -> Result<(ResultAndState<HaltReasonFor<Self::Evm>>, EvmEnvFor<Self::Evm>), Self::Error> {
        let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

        if let Some(bal) = bal {
            // the BAL also covers the block's pre-execution changes
            attach_bal_before_tx(db, bal, target_tx_index);
            let mut evm = self.evm_config().evm_with_env_and_inspector(db, evm_env, inspector);
            let res = evm.transact(target_tx_env).map_err(Self::Error::from_evm_err)?;
            let (_, evm_env) = evm.finish();
            return Ok((res, evm_env))
        }

        let mut executor = self.inspected_block_executor(block, db, evm_env.clone(), inspector)?;

        let mut block_txs = block.transactions_recovered();
        for tx in block_txs.by_ref().take(target_tx_index) {
            executor.execute_transaction(tx).map_err(Self::Error::from_eth_err)?;
        }
        let target_tx = block_txs.next().ok_or_else(|| {
            Self::Error::from_eth_err(EthApiError::InvalidParams(format!(
                "tx_index {target_tx_index} out of bounds for block with {} transactions",
                block.transaction_count()
            )))
        })?;

        executor.evm_mut().enable_inspector();
        let res = executor
            .execute_transaction_without_commit(target_tx)
            .map_err(Self::Error::from_eth_err)?
            .into_result();

        Ok((res, evm_env))
    }

    /// Creates a block executor for `block` whose EVM runs the given inspector, and applies the
    /// block's pre-execution changes. The inspector is left disabled.
    ///
    /// Block transactions must be replayed through the executor rather than its EVM: the executor
    /// tracks the transaction's position in the block and applies chain-specific per-transaction
    /// settlement, such as OP Stack's post-execution gas refunds, that a bare EVM replay misses.
    #[expect(clippy::type_complexity, reason = "the executor type is spelled out by its factory")]
    fn inspected_block_executor<'a, 'db, I>(
        &'a self,
        block: &'a RecoveredBlock<BlockTy<Self::Primitives>>,
        db: &'db mut StateCacheDb,
        evm_env: EvmEnvFor<Self::Evm>,
        inspector: I,
    ) -> Result<
        BlockExecutorFor<
            'a,
            <Self::Evm as ConfigureEvm>::BlockExecutorFactory,
            &'db mut StateCacheDb,
            I,
        >,
        Self::Error,
    >
    where
        I: InspectorFor<Self::Evm, &'db mut StateCacheDb>,
    {
        let evm_config = self.evm_config();
        let ctx = evm_config
            .context_for_block(block.sealed_block())
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;
        let mut evm = evm_config.evm_with_env_and_inspector(db, evm_env, inspector);
        evm.disable_inspector();

        let mut executor = evm_config.create_executor_with_state(evm, ctx);
        executor.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;
        Ok(executor)
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

                // prepare transactions, we do everything upfront to reduce time spent with open
                // state
                let max_transactions = highest_index
                    .map_or_else(
                        || block.body().transaction_count(),
                        |highest| {
                            // we need + 1 because the index is 0-based
                            (highest as usize).saturating_add(1)
                        },
                    )
                    .min(block.body().transaction_count());

                let inspector = inspector_setup();
                let fused_inspector = inspector.clone();
                let mut executor =
                    this.inspected_block_executor(&block, &mut db, evm_env, inspector)?;
                executor.evm_mut().enable_inspector();

                let mut results = Vec::with_capacity(max_transactions);
                let mut transactions =
                    block.transactions_recovered().take(max_transactions).enumerate().peekable();
                while let Some((idx, tx)) = transactions.next() {
                    if is_cancelled() {
                        return Err(EthApiError::InternalEthError.into())
                    }
                    let output = executor
                        .execute_transaction_without_commit(tx)
                        .map_err(Self::Error::from_eth_err)?;
                    let ResultAndState { result, state } = output.result();

                    let tx_info = TransactionInfo {
                        hash: Some(*tx.tx_hash()),
                        index: Some(idx as u64),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        block_timestamp: Some(block_timestamp),
                        base_fee: Some(base_fee),
                    };
                    let mut was_fused = false;
                    let (db, inspector, _) = executor.evm_mut().components_mut();
                    results.push(f(
                        tx_info,
                        TracingCtx {
                            tx,
                            result,
                            state,
                            inspector,
                            db,
                            fused_inspector: &fused_inspector,
                            was_fused: &mut was_fused,
                        },
                    )?);
                    // The inspector and state of the last traced transaction are not needed.
                    if transactions.peek().is_some() {
                        if !was_fused {
                            *executor.evm_mut().inspector_mut() = fused_inspector.clone();
                        }
                        executor.commit_transaction(output);
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
    ///    _after_ the transaction [`StateCacheDb`] and the database that points to the state right
    ///    _before_ the transaction.
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
            .map_err(Self::Error::from_eth_err)?;
        Ok(())
    }
}

/// Context for one transaction of a block traced by [`Trace::trace_block_until_with_inspector`].
#[derive(Debug)]
pub struct TracingCtx<'a, T, E: Evm> {
    /// The transaction that was just executed.
    pub tx: T,
    /// Result of transaction execution.
    pub result: &'a ExecutionResult<E::HaltReason>,
    /// State changes after transaction.
    pub state: &'a EvmState,
    /// Inspector state after transaction.
    pub inspector: &'a mut E::Inspector,
    /// Database used when executing the transaction, _before_ committing the state changes.
    pub db: &'a mut E::DB,
    /// Inspector state before any transaction was executed.
    fused_inspector: &'a E::Inspector,
    /// Whether the inspector was already reset by [`TracingCtx::take_inspector`].
    was_fused: &'a mut bool,
}

impl<T, E: Evm<Inspector: Clone>> TracingCtx<'_, T, E> {
    /// Returns the inspector state of this transaction, resetting the inspector for the next one.
    pub fn take_inspector(&mut self) -> E::Inspector {
        *self.was_fused = true;
        core::mem::replace(self.inspector, self.fused_inspector.clone())
    }
}
