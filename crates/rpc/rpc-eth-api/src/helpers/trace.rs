//! Loads a pending block from database. Helper trait for `eth_` call and trace RPC methods.

use super::{Call, LoadBlock, LoadState, LoadTransaction};
use crate::{FromEthApiError, FromEvmError};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::B256;
use alloy_rpc_types_eth::{BlockId, TransactionInfo};
use alloy_rpc_types_trace::geth::{GethDebugTracingOptions, TraceResult};
use evm2::evm::{Bal as EvmBal, StateChangeSource};
use evm2_inspectors::tracing::{
    DebugInspector, TracingInspector, TracingInspectorConfig, TransactionContext,
};
use futures::Future;
use reth_errors::RethError;
use reth_evm::{
    ConfigureEvm, Database, Evm, EvmEnv, EvmEnvFor, EvmTypesFor, TxEnvFor, TxResultWithStateFor,
};
use reth_primitives_traits::{BlockBody, BlockTy, RecoveredBlock};
use reth_rpc_eth_types::{cache::db::attach_bal_before_tx, EthApiError, StateCacheDb};
use reth_storage_api::{HashedPostStateProvider, ProviderBlock, StateRootProvider};
use reth_tasks::cancel::is_cancelled;
use std::sync::Arc;

/// Context passed to per-transaction trace callbacks.
pub struct TracingCtx<'a, Insp, EvmTypes: evm2::EvmTypes> {
    /// Execution result and detached state changes for the transaction.
    pub result: &'a evm2::TxResultWithState<EvmTypes>,
    /// Database pointing to the transaction pre-state.
    pub db: &'a mut dyn evm2::evm::DynDatabase,
    /// Inspector used while executing the transaction.
    pub inspector: Insp,
}

impl<Insp, EvmTypes: evm2::EvmTypes> core::fmt::Debug for TracingCtx<'_, Insp, EvmTypes> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TracingCtx").finish_non_exhaustive()
    }
}

impl<Insp, EvmTypes: evm2::EvmTypes> TracingCtx<'_, Insp, EvmTypes> {
    /// Consumes this context and returns the inspector.
    pub fn take_inspector(self) -> Insp {
        self.inspector
    }
}

/// Executes CPU heavy trace tasks.
pub trait Trace: LoadState<Error: FromEvmError<Self::Evm>> + Call {
    /// Executes a transaction with the provided inspector without committing state changes.
    fn inspect<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: &TxEnvFor<Self::Evm>,
        inspector: I,
    ) -> Result<(I, TxResultWithStateFor<Self::Evm>), Self::Error>
    where
        DB: Database,
        I: evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
    {
        let mut evm = self.evm_config().evm_with_env(db, evm_env);
        evm.transact_with_inspector(tx_env, inspector).map_err(Self::Error::from_evm_err)
    }

    /// Executes the transaction on top of the given [`BlockId`] with a tracer configured by the
    /// config and provides the used database to the callback.
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
                TxResultWithStateFor<Self::Evm>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.spawn_with_state_at_block(at, move |this, mut db| {
            let (inspector, result) =
                this.inspect(&mut db, evm_env, &tx_env, TracingInspector::new(config))?;
            f(inspector, result, db)
        })
    }

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, the state is positioned right before the transaction,
    /// either by attaching the block's cached BAL or by executing all previous transactions in
    /// the block.
    /// The callback `f` is invoked with the [`TxResultWithStateFor`] after the transaction was
    /// executed and the database that points to the beginning of the transaction. The database may
    /// have the block's BAL attached and must only be used for reads, because an attached BAL takes
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
                TxResultWithStateFor<Self::Evm>,
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
    /// the block.
    /// The callback `f` is invoked with the [`TxResultWithStateFor`] after the transaction was
    /// executed and the database that points to the beginning of the transaction. The database may
    /// have the block's BAL attached and must only be used for reads, because an attached BAL takes
    /// read precedence over state committed on top, see [`attach_bal_before_tx`].
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
                TxResultWithStateFor<Self::Evm>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Insp: evm2::Inspector<EvmTypesFor<Self::Evm>> + Send + 'static,
        R: Send + 'static,
    {
        async move {
            let Some((transaction, block, bal)) =
                self.transaction_and_block_and_maybe_bal(hash).await?
            else {
                return Ok(None);
            };
            let (_, tx_info) = transaction.split();

            // we need to get the state of the parent block because we're essentially replaying the
            // block the transaction is included in
            self.spawn_with_state_at_block(block.parent_hash(), move |this, mut db| {
                let (inspector, result, _) = this.inspect_canonical_transaction_in_block(
                    &block,
                    &mut db,
                    inspector,
                    // index should always be available because `transaction_and_block` only
                    // returns transactions included in a block
                    tx_info.index.expect("transaction_and_block only returns block transactions")
                        as usize,
                    bal.as_deref(),
                )?;
                f(tx_info, inspector, result, db)
            })
            .await
            .map(Some)
        }
    }

    /// Positions the state of `db` right before the transaction at the target index.
    ///
    /// If the block's cached BAL is given, it is attached to the database at the target index and
    /// no transactions are executed, see [`attach_bal_before_tx`]. Otherwise all transactions
    /// before the target transaction are executed and their changes are written to the
    /// _runtime_ db ([`StateCacheDb`]).
    ///
    /// If the target index is greater than or equal to the block's transaction count, all
    /// transactions are replayed.
    fn replay_block_until(
        &self,
        db: &mut StateCacheDb,
        block: &RecoveredBlock<BlockTy<Self::Primitives>>,
        target_tx_index: usize,
        bal: Option<&DecodedBal<Arc<EvmBal>>>,
    ) -> Result<(), Self::Error> {
        if let Some(bal) = bal {
            attach_bal_before_tx(db, bal, target_tx_index);
            return Ok(())
        }

        self.apply_pre_execution_changes(block, db)?;

        let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;
        let mut evm = self.evm_config().evm_with_env(&mut *db, evm_env);
        self.replay_transactions_until_with_evm(
            &mut evm,
            block.transactions_recovered(),
            target_tx_index,
        )?;
        let cache = evm.take_state_cache();
        drop(evm);
        db.cache.merge(cache);
        Ok(())
    }

    /// Executes the target transaction with the configured inspector on the state right before
    /// the transaction.
    ///
    /// If the block's cached BAL is given, the state is positioned by attaching the BAL at the
    /// target index, see [`attach_bal_before_tx`]. Otherwise all transactions before the target
    /// transaction are replayed without inspection first.
    #[expect(clippy::type_complexity)]
    fn inspect_transaction_in_block<I>(
        &self,
        block: &RecoveredBlock<BlockTy<Self::Primitives>>,
        db: &mut StateCacheDb,
        inspector: I,
        target_tx_index: usize,
        target_tx_env: &TxEnvFor<Self::Evm>,
        bal: Option<&DecodedBal<Arc<EvmBal>>>,
    ) -> Result<(I, TxResultWithStateFor<Self::Evm>, EvmEnvFor<Self::Evm>), Self::Error>
    where
        I: evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
    {
        if let Some(bal) = bal {
            // the BAL also covers the block's pre-execution changes
            attach_bal_before_tx(db, bal, target_tx_index);
        } else {
            self.apply_pre_execution_changes(block, db)?;
        }

        let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;
        let mut evm = self.evm_config().evm_with_env(&mut *db, evm_env.clone());

        if bal.is_none() {
            self.replay_transactions_until_with_evm(
                &mut evm,
                block.transactions_recovered(),
                target_tx_index,
            )?;
        }

        let (inspector, result) = evm
            .transact_with_inspector(target_tx_env, inspector)
            .map_err(Self::Error::from_evm_err)?;
        let cache = evm.take_state_cache();
        drop(evm);
        db.cache.merge(cache);
        Ok((inspector, result, evm_env))
    }

    /// Executes the signed transaction recorded at `target_tx_index` in `block` with the
    /// configured inspector on the state right before it, see
    /// [`Trace::inspect_transaction_in_block`].
    ///
    /// Returns [`EthApiError::InvalidParams`] if the block has no transaction at that index.
    #[expect(clippy::type_complexity)]
    fn inspect_canonical_transaction_in_block<I>(
        &self,
        block: &RecoveredBlock<BlockTy<Self::Primitives>>,
        db: &mut StateCacheDb,
        inspector: I,
        target_tx_index: usize,
        bal: Option<&DecodedBal<Arc<EvmBal>>>,
    ) -> Result<(I, TxResultWithStateFor<Self::Evm>, EvmEnvFor<Self::Evm>), Self::Error>
    where
        I: evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
    {
        let Some(tx) = block.transactions_recovered().nth(target_tx_index) else {
            return Err(EthApiError::InvalidParams(format!(
                "tx_index {target_tx_index} out of bounds for block with {} transactions",
                block.transaction_count()
            ))
            .into())
        };
        let tx_env = self.evm_config().tx_env(tx.cloned());
        self.inspect_transaction_in_block(block, db, inspector, target_tx_index, &tx_env, bal)
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
                TracingCtx<'_, TracingInspector, EvmTypesFor<Self::Evm>>,
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
        inspector_setup: Setup,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        F: Fn(
                TransactionInfo,
                TracingCtx<'_, Insp, EvmTypesFor<Self::Evm>>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
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
                // prepare transactions, we do everything upfront to reduce time spent with open
                // state
                let max_transactions = highest_index.map_or_else(
                    || block.body().transaction_count(),
                    |highest| {
                        // we need + 1 because the index is 0-based
                        highest as usize + 1
                    },
                );

                this.trace_block_transactions(
                    &mut db,
                    &block,
                    evm_env,
                    max_transactions,
                    inspector_setup,
                    f,
                )
                .map(Some)
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
        F: Fn(
                TransactionInfo,
                TracingCtx<'_, TracingInspector, EvmTypesFor<Self::Evm>>,
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
        F: Fn(
                TransactionInfo,
                TracingCtx<'_, Insp, EvmTypesFor<Self::Evm>>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
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
    ) -> Result<reth_trie_common::bundle_state::BundleState, Self::Error> {
        let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;
        let ctx = self
            .evm_config()
            .context_for_block(block.sealed_block())
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;
        let changes = self
            .evm_config()
            .pre_block_state_changes(&mut *db, evm_env, ctx)
            .map_err(|err| EthApiError::EvmCustom(err.to_string()))
            .map_err(Self::Error::from_eth_err)?;
        db.commit_source(&reth_execution_types::BundleSource(&changes));
        Ok(changes)
    }

    /// Executes the first `max_transactions` transactions of `block` on `db`, which holds the
    /// state of the block's parent, and invokes `f` with each transaction's [`TracingCtx`].
    ///
    /// Backs [`Trace::trace_block_until_with_inspector`]: `inspector_setup` is invoked once per
    /// transaction, and the changes of the executed transactions are not merged back into `db`.
    fn trace_block_transactions<Setup, Insp, F, R>(
        &self,
        db: &mut StateCacheDb,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        evm_env: EvmEnvFor<Self::Evm>,
        max_transactions: usize,
        mut inspector_setup: Setup,
        f: F,
    ) -> Result<Vec<R>, Self::Error>
    where
        F: Fn(
            TransactionInfo,
            TracingCtx<'_, Insp, EvmTypesFor<Self::Evm>>,
        ) -> Result<R, Self::Error>,
        Setup: FnMut() -> Insp,
        Insp: evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
    {
        let block_hash = block.hash();

        let block_number = block.number();
        let block_timestamp = block.timestamp();
        let base_fee = block.base_fee_per_gas();

        self.apply_pre_execution_changes(block, db)?;

        let tx_count = max_transactions.min(block.body().transaction_count());

        let mut results = Vec::new();
        let mut evm = self.evm_config().evm_with_env(&mut *db, evm_env);

        for (idx, tx) in block.transactions_recovered().take(max_transactions).enumerate() {
            if is_cancelled() {
                return Err(EthApiError::InternalEthError.into())
            }
            let tx_env = self.evm_config().tx_env(tx.cloned());
            let (inspector, result) = evm
                .transact_with_inspector(&tx_env, inspector_setup())
                .map_err(Self::Error::from_evm_err)?;
            let tx_info = TransactionInfo {
                hash: Some(*tx.tx_hash()),
                index: Some(idx as u64),
                block_hash: Some(block_hash),
                block_number: Some(block_number),
                block_timestamp: Some(block_timestamp),
                base_fee,
            };
            let item =
                f(tx_info, TracingCtx { result: &result, db: evm.state_db_mut(), inspector })?;
            // the EVM is dropped after the last transaction, so its state needs no commit
            if idx + 1 < tx_count {
                evm.commit_state(&result.pending_state);
            }
            results.push(item);
        }

        Ok(results)
    }

    /// Traces every transaction of a block the node has validated, on `db`, which holds the
    /// state of the block's parent.
    ///
    /// Backs `debug_traceBlockByHash` and `debug_traceBlockByNumber`. Chains whose block execution
    /// changes state beyond the transactions themselves can override this to replay the block
    /// through their block executor, while
    /// [`Trace::debug_trace_unvalidated_block_transactions`] keeps serving blocks such an executor
    /// could reject.
    fn debug_trace_block_transactions(
        &self,
        db: &mut StateCacheDb,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        evm_env: EvmEnvFor<Self::Evm>,
        opts: GethDebugTracingOptions,
    ) -> Result<Vec<TraceResult>, Self::Error> {
        self.debug_trace_unvalidated_block_transactions(db, block, evm_env, opts)
    }

    /// Traces every transaction of a caller-supplied or rejected block on `db`, which holds the
    /// state of the block's parent, without block-level validation.
    ///
    /// Backs `debug_traceBlock` and `debug_traceBadBlock`.
    fn debug_trace_unvalidated_block_transactions(
        &self,
        db: &mut StateCacheDb,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        evm_env: EvmEnvFor<Self::Evm>,
        opts: GethDebugTracingOptions,
    ) -> Result<Vec<TraceResult>, Self::Error> {
        let mut results = Vec::with_capacity(block.body().transactions().len());

        self.apply_pre_execution_changes(block, db)?;

        let block_env = *evm_env.block_env();

        let mut transactions = block.transactions_recovered().enumerate().peekable();
        let mut inspector = DebugInspector::new(opts).map_err(Self::Error::from_eth_err)?;
        let mut evm = self.evm_config().evm_with_env(&mut *db, evm_env);
        while let Some((index, tx)) = transactions.next() {
            if is_cancelled() {
                return Err(EthApiError::InternalEthError.into())
            }
            let tx_env = self.evm_config().tx_env(tx.cloned());

            let (next_inspector, res) = evm
                .transact_with_inspector(&tx_env, inspector)
                .map_err(Self::Error::from_evm_err)?;
            inspector = next_inspector;
            let result = inspector
                .get_result(
                    Some(TransactionContext {
                        block_hash: Some(block.hash()),
                        tx_hash: Some(*tx.tx_hash()),
                        tx_index: Some(index),
                    }),
                    &tx_env,
                    &block_env,
                    &res,
                    evm.state_db_mut(),
                )
                .map_err(Self::Error::from_eth_err)?;

            results.push(TraceResult::Success { result, tx_hash: Some(*tx.tx_hash()) });
            if transactions.peek().is_some() {
                inspector.fuse().map_err(Self::Error::from_eth_err)?;
                // need to apply the state changes of this transaction before executing the
                // next transaction
                evm.commit_state(&res.pending_state)
            }
        }

        Ok(results)
    }

    /// Traces every transaction of a block for `debug_traceChain` on `db`, which holds the state
    /// of the block's parent.
    ///
    /// A transaction that fails to trace is reported as [`TraceResult::Error`] and ends the
    /// block; the entries for the transactions after it are `None`.
    fn debug_trace_chain_block_transactions(
        &self,
        db: &mut StateCacheDb,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        evm_env: EvmEnvFor<Self::Evm>,
        opts: GethDebugTracingOptions,
    ) -> Result<Vec<Option<TraceResult>>, Self::Error> {
        let tx_count = block.body().transactions().len();
        let mut results = Vec::with_capacity(tx_count);

        self.apply_pre_execution_changes(block, db)?;

        let block_env = *evm_env.block_env();
        let mut transactions = block.transactions_recovered().enumerate().peekable();
        let mut inspector = match DebugInspector::new(opts) {
            Ok(inspector) => inspector,
            Err(err) => {
                if let Some((_, tx)) = transactions.peek() {
                    results.push(Some(TraceResult::Error {
                        error: err.to_string(),
                        tx_hash: Some(*tx.tx_hash()),
                    }));
                }
                results.resize(tx_count, None);
                return Ok(results)
            }
        };
        let mut evm = self.evm_config().evm_with_env(&mut *db, evm_env);

        while let Some((index, tx)) = transactions.next() {
            let tx_hash = *tx.tx_hash();
            let tx_env = self.evm_config().tx_env(tx.cloned());
            let res = match evm.transact_with_inspector(&tx_env, inspector) {
                Ok((next_inspector, res)) => {
                    inspector = next_inspector;
                    res
                }
                Err(err) => {
                    results.push(Some(TraceResult::Error {
                        error: err.to_string(),
                        tx_hash: Some(tx_hash),
                    }));
                    break
                }
            };

            let result = match inspector.get_result(
                Some(TransactionContext {
                    block_hash: Some(block.hash()),
                    tx_hash: Some(tx_hash),
                    tx_index: Some(index),
                }),
                &tx_env,
                &block_env,
                &res,
                evm.state_db_mut(),
            ) {
                Ok(result) => result,
                Err(err) => {
                    results.push(Some(TraceResult::Error {
                        error: err.to_string(),
                        tx_hash: Some(tx_hash),
                    }));
                    break
                }
            };

            results.push(Some(TraceResult::Success { result, tx_hash: Some(tx_hash) }));
            if let Some((_, next_tx)) = transactions.peek() {
                if let Err(err) = inspector.fuse() {
                    results.push(Some(TraceResult::Error {
                        error: err.to_string(),
                        tx_hash: Some(*next_tx.tx_hash()),
                    }));
                    break
                }
                evm.commit_state(&res.pending_state);
            }
        }

        results.resize(tx_count, None);
        Ok(results)
    }

    /// Executes every transaction of `block` on `db`, which holds the state of the block's
    /// parent, and returns the state root after each one.
    ///
    /// Backs `debug_intermediateRoots`.
    fn block_intermediate_roots(
        &self,
        db: &mut StateCacheDb,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        evm_env: EvmEnvFor<Self::Evm>,
    ) -> Result<Vec<B256>, Self::Error> {
        let mut roots = Vec::with_capacity(block.body().transactions().len());
        let mut state = self.apply_pre_execution_changes(block, db)?;
        for tx in block.transactions_recovered() {
            if is_cancelled() {
                return Err(EthApiError::InternalEthError.into())
            }
            let tx_env = self.evm_config().tx_env(tx.cloned());
            let result = self.transact(&mut *db, evm_env.clone(), tx_env)?;
            let mut changes = reth_execution_types::TransactionChanges::default();
            let Ok(()) = result.pending_state.visit(&mut changes);
            let mut transaction_state = reth_execution_types::BlockState::default();
            transaction_state.commit(&changes);
            state.extend(transaction_state.into_bundle());
            db.commit_source(&result.pending_state);
            let hashed_state =
                db.db.inner().hashed_post_state(&state).map_err(Self::Error::from_eth_err)?;
            let root = db.db.inner().state_root(hashed_state).map_err(Self::Error::from_eth_err)?;
            roots.push(root);
        }

        Ok(roots)
    }
}
