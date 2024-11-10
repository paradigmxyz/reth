//! Loads a pending block from database. Helper trait for `eth_` call and trace RPC methods.

use std::{fmt::Display, sync::Arc};

use crate::{FromEvmError, RpcNodeCore};
use alloy_primitives::B256;
use alloy_rpc_types_eth::{BlockId, TransactionInfo};
use futures::Future;
use reth_chainspec::ChainSpecProvider;
use reth_evm::{system_calls::SystemCaller, ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{Header, SealedBlockWithSenders};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_eth_types::{
    cache::db::{StateCacheDb, StateCacheDbRefMutWrapper, StateProviderTraitObjWrapper},
    EthApiError,
};
use revm::{db::CacheDB, Database, DatabaseCommit, GetInspector, Inspector};
use revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use revm_primitives::{
    BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, EvmState, ExecutionResult, ResultAndState,
};

use super::{Call, LoadBlock, LoadPendingBlock, LoadState, LoadTransaction};

/// Executes CPU heavy tasks.
pub trait Trace: LoadState<Evm: ConfigureEvm<Header = Header>> {
    /// Executes the [`EnvWithHandlerCfg`] against the given [Database] without committing state
    /// changes.
    fn inspect<DB, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> Result<(ResultAndState, EnvWithHandlerCfg), Self::Error>
    where
        DB: Database,
        EthApiError: From<DB::Error>,
        I: GetInspector<DB>,
    {
        self.inspect_and_return_db(db, env, inspector).map(|(res, env, _)| (res, env))
    }

    /// Same as [`inspect`](Self::inspect) but also returns the database again.
    ///
    /// Even though [Database] is also implemented on `&mut`
    /// this is still useful if there are certain trait bounds on the Inspector's database generic
    /// type
    fn inspect_and_return_db<DB, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> Result<(ResultAndState, EnvWithHandlerCfg, DB), Self::Error>
    where
        DB: Database,
        EthApiError: From<DB::Error>,

        I: GetInspector<DB>,
    {
        let mut evm = self.evm_config().evm_with_env_and_inspector(db, env, inspector);
        let res = evm.transact().map_err(Self::Error::from_evm_err)?;
        let (db, env) = evm.into_db_and_env_with_handler_cfg();
        Ok((res, env, db))
    }

    /// Executes the transaction on top of the given [`BlockId`] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [`TracingInspector`] and the [`ResultAndState`] after
    /// the configured [`EnvWithHandlerCfg`] was inspected.
    ///
    /// Caution: this is blocking
    fn trace_at<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> Result<R, Self::Error>
    where
        Self: Call,
        F: FnOnce(TracingInspector, ResultAndState) -> Result<R, Self::Error>,
    {
        self.with_state_at_block(at, |state| {
            let mut db = CacheDB::new(StateProviderDatabase::new(state));
            let mut inspector = TracingInspector::new(config);
            let (res, _) = self.inspect(&mut db, env, &mut inspector)?;
            f(inspector, res)
        })
    }

    /// Same as [`trace_at`](Self::trace_at) but also provides the used database to the callback.
    ///
    /// Executes the transaction on top of the given [`BlockId`] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [`TracingInspector`] and the [`ResultAndState`] after
    /// the configured [`EnvWithHandlerCfg`] was inspected.
    fn spawn_trace_at_with_state<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        Self: LoadPendingBlock + Call,
        F: FnOnce(TracingInspector, ResultAndState, StateCacheDb<'_>) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        let this = self.clone();
        self.spawn_with_state_at_block(at, move |state| {
            let mut db = CacheDB::new(StateProviderDatabase::new(state));
            let mut inspector = TracingInspector::new(config);
            let (res, _) = this.inspect(&mut db, env, &mut inspector)?;
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
        Self: LoadPendingBlock + LoadTransaction + Call,
        F: FnOnce(
                TransactionInfo,
                TracingInspector,
                ResultAndState,
                StateCacheDb<'_>,
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
        Self: LoadPendingBlock + LoadTransaction + Call,
        F: FnOnce(
                TransactionInfo,
                Insp,
                ResultAndState,
                StateCacheDb<'_>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Insp: for<'a, 'b> Inspector<StateCacheDbRefMutWrapper<'a, 'b>> + Send + 'static,
        R: Send + 'static,
    {
        async move {
            let (transaction, block) = match self.transaction_and_block(hash).await? {
                None => return Ok(None),
                Some(res) => res,
            };
            let (tx, tx_info) = transaction.split();

            let (cfg, block_env, _) = self.evm_env_at(block.hash().into()).await?;

            // we need to get the state of the parent block because we're essentially replaying the
            // block the transaction is included in
            let parent_block = block.parent_hash;

            let this = self.clone();
            self.spawn_with_state_at_block(parent_block.into(), move |state| {
                let mut db = CacheDB::new(StateProviderDatabase::new(state));
                let block_txs = block.transactions_with_sender();

                this.apply_pre_execution_changes(&block, &mut db, &cfg, &block_env)?;

                // replay all transactions prior to the targeted transaction
                this.replay_transactions_until(
                    &mut db,
                    cfg.clone(),
                    block_env.clone(),
                    block_txs,
                    tx.hash,
                )?;

                let env = EnvWithHandlerCfg::new_with_cfg_env(
                    cfg,
                    block_env,
                    RpcNodeCore::evm_config(&this).tx_env(tx.as_signed(), tx.signer()),
                );
                let (res, _) =
                    this.inspect(StateCacheDbRefMutWrapper(&mut db), env, &mut inspector)?;
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
        block: Option<Arc<SealedBlockWithSenders>>,
        highest_index: Option<u64>,
        config: TracingInspectorConfig,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        F: Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &EvmState,
                &StateCacheDb<'_>,
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
        block: Option<Arc<SealedBlockWithSenders>>,
        highest_index: Option<u64>,
        mut inspector_setup: Setup,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        F: Fn(
                TransactionInfo,
                Insp,
                ExecutionResult,
                &EvmState,
                &StateCacheDb<'_>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: for<'a, 'b> Inspector<StateCacheDbRefMutWrapper<'a, 'b>> + Send + 'static,
        R: Send + 'static,
    {
        async move {
            let block = async {
                if block.is_some() {
                    return Ok(block)
                }
                self.block_with_senders(block_id).await
            };

            let ((cfg, block_env, _), block) =
                futures::try_join!(self.evm_env_at(block_id), block)?;

            let Some(block) = block else { return Ok(None) };

            if block.body.transactions.is_empty() {
                // nothing to trace
                return Ok(Some(Vec::new()))
            }

            // replay all transactions of the block
            self.spawn_tracing(move |this| {
                // we need to get the state of the parent block because we're replaying this block
                // on top of its parent block's state
                let state_at = block.parent_hash;
                let block_hash = block.hash();

                let block_number = block_env.number.saturating_to::<u64>();
                let base_fee = block_env.basefee.saturating_to::<u128>();

                // now get the state
                let state = this.state_at_block_id(state_at.into())?;
                let mut db =
                    CacheDB::new(StateProviderDatabase::new(StateProviderTraitObjWrapper(&state)));

                this.apply_pre_execution_changes(&block, &mut db, &cfg, &block_env)?;

                // prepare transactions, we do everything upfront to reduce time spent with open
                // state
                let max_transactions =
                    highest_index.map_or(block.body.transactions.len(), |highest| {
                        // we need + 1 because the index is 0-based
                        highest as usize + 1
                    });
                let mut results = Vec::with_capacity(max_transactions);

                let mut transactions = block
                    .transactions_with_sender()
                    .take(max_transactions)
                    .enumerate()
                    .map(|(idx, (signer, tx))| {
                        let tx_info = TransactionInfo {
                            hash: Some(tx.hash()),
                            index: Some(idx as u64),
                            block_hash: Some(block_hash),
                            block_number: Some(block_number),
                            base_fee: Some(base_fee),
                        };
                        let tx_env = this.evm_config().tx_env(tx, *signer);
                        (tx_info, tx_env)
                    })
                    .peekable();

                while let Some((tx_info, tx)) = transactions.next() {
                    let env =
                        EnvWithHandlerCfg::new_with_cfg_env(cfg.clone(), block_env.clone(), tx);

                    let mut inspector = inspector_setup();
                    let (res, _) =
                        this.inspect(StateCacheDbRefMutWrapper(&mut db), env, &mut inspector)?;
                    let ResultAndState { result, state } = res;
                    results.push(f(tx_info, inspector, result, &state, &db)?);

                    // need to apply the state changes of this transaction before executing the
                    // next transaction, but only if there's a next transaction
                    if transactions.peek().is_some() {
                        // commit the state changes to the DB
                        db.commit(state)
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
    /// 2. configures the EVM evn
    /// 3. loops over all transactions and executes them
    /// 4. calls the callback with the transaction info, the execution result, the changed state
    ///    _after_ the transaction [`StateProviderDatabase`] and the database that points to the
    ///    state right _before_ the transaction.
    fn trace_block_with<F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<SealedBlockWithSenders>>,
        config: TracingInspectorConfig,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        // This is the callback that's invoked for each transaction with the inspector, the result,
        // state and db
        F: Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &EvmState,
                &StateCacheDb<'_>,
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
    /// 2. configures the EVM evn
    /// 3. loops over all transactions and executes them
    /// 4. calls the callback with the transaction info, the execution result, the changed state
    ///    _after_ the transaction [`EvmState`] and the database that points to the state right
    ///    _before_ the transaction, in other words the state the transaction was executed on:
    ///    `changed_state = tx(cached_state)`
    ///
    /// This accepts a `inspector_setup` closure that returns the inspector to be used for tracing
    /// a transaction. This is invoked for each transaction.
    fn trace_block_inspector<Setup, Insp, F, R>(
        &self,
        block_id: BlockId,
        block: Option<Arc<SealedBlockWithSenders>>,
        insp_setup: Setup,
        f: F,
    ) -> impl Future<Output = Result<Option<Vec<R>>, Self::Error>> + Send
    where
        Self: LoadBlock,
        // This is the callback that's invoked for each transaction with the inspector, the result,
        // state and db
        F: Fn(
                TransactionInfo,
                Insp,
                ExecutionResult,
                &EvmState,
                &StateCacheDb<'_>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: for<'a, 'b> Inspector<StateCacheDbRefMutWrapper<'a, 'b>> + Send + 'static,
        R: Send + 'static,
    {
        self.trace_block_until_with_inspector(block_id, block, None, insp_setup, f)
    }

    /// Applies chain-specific state transitions required before executing a block.
    ///
    /// Note: This should only be called when tracing an entire block vs individual transactions.
    /// When tracing transaction on top of an already committed block state, those transitions are
    /// already applied.
    fn apply_pre_execution_changes<DB: Send + Database<Error: Display> + DatabaseCommit>(
        &self,
        block: &SealedBlockWithSenders,
        db: &mut DB,
        cfg: &CfgEnvWithHandlerCfg,
        block_env: &BlockEnv,
    ) -> Result<(), Self::Error> {
        let mut system_caller =
            SystemCaller::new(self.evm_config().clone(), self.provider().chain_spec());
        // apply relevant system calls
        system_caller
            .pre_block_beacon_root_contract_call(
                db,
                cfg,
                block_env,
                block.header.parent_beacon_block_root,
            )
            .map_err(|_| EthApiError::EvmCustom("failed to apply 4788 system call".to_string()))?;

        system_caller
            .pre_block_blockhashes_contract_call(db, cfg, block_env, block.header.parent_hash)
            .map_err(|_| {
                EthApiError::EvmCustom("failed to apply blockhashes system call".to_string())
            })?;

        Ok(())
    }
}
