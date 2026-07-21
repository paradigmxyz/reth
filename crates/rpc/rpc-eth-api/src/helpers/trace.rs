//! Loads a pending block from database. Helper trait for `eth_` call and trace RPC methods.

use super::{Call, LoadBlock, LoadState, LoadTransaction};
use crate::{FromEthApiError, FromEvmError};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_primitives::B256;
use alloy_rpc_types_eth::{BlockId, TransactionInfo};
use evm2_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use futures::Future;
use reth_errors::RethError;
use reth_evm::{
    execute::BlockExecutorFactory, BlockExecutor, ConfigureEvm, Database, Evm, EvmEnv, EvmEnvFor,
    EvmTypesFor, TxEnvFor, TxResultWithStateFor,
};
use reth_primitives_traits::{BlockBody, Recovered, RecoveredBlock};
use reth_rpc_eth_types::StateCacheDb;
use reth_storage_api::{ProviderBlock, ProviderTx};
use std::{mem, sync::Arc};

/// Context passed to per-transaction trace callbacks.
pub struct TracingCtx<'a, Tx, Insp, EvmTypes: evm2::EvmTypes> {
    /// Transaction being traced.
    pub tx: Tx,
    /// Execution result for the transaction.
    pub result: evm2::TxResult<EvmTypes>,
    /// Detached state changes for the transaction.
    pub state: &'a evm2::PendingState,
    /// Database pointing to the transaction pre-state.
    pub db: &'a mut dyn evm2::evm::DynDatabase,
    /// Inspector used while executing the transaction.
    pub inspector: &'a mut Insp,
    fused_inspector: &'a Insp,
    was_fused: &'a mut bool,
}

impl<Tx, Insp, EvmTypes: evm2::EvmTypes> core::fmt::Debug for TracingCtx<'_, Tx, Insp, EvmTypes> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TracingCtx").finish_non_exhaustive()
    }
}

impl<Tx, Insp: Clone, EvmTypes: evm2::EvmTypes> TracingCtx<'_, Tx, Insp, EvmTypes> {
    /// Returns the inspector, leaving a pristine inspector for the next transaction.
    pub fn take_inspector(&mut self) -> Insp {
        *self.was_fused = true;
        mem::replace(self.inspector, self.fused_inspector.clone())
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
        inspector: &mut I,
    ) -> Result<TxResultWithStateFor<Self::Evm>, Self::Error>
    where
        DB: Database,
        I: evm2::Inspector<EvmTypesFor<Self::Evm>>,
    {
        let mut evm = self.evm_config().block_executor_factory().evm_with_database(db, evm_env);
        evm.transact_with_inspector(tx_env, inspector).map_err(Self::Error::from_evm_err)
    }

    /// Executes the transaction on top of the given [`BlockId`] with a tracer configured by the
    /// config.
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
        F: FnOnce(TracingInspector, TxResultWithStateFor<Self::Evm>) -> Result<R, Self::Error>
            + Send
            + 'static,
    {
        self.spawn_with_state_at_block(at, move |this, mut db| {
            let mut inspector = TracingInspector::new(config);
            let result = this.inspect(&mut db, evm_env, &tx_env, &mut inspector)?;
            f(inspector, result)
        })
    }

    /// Same as [`trace_at`](Self::trace_at) but also provides the used database to the callback.
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
            let mut inspector = TracingInspector::new(config);
            let result = this.inspect(&mut db, evm_env, &tx_env, &mut inspector)?;
            f(inspector, result, db)
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

    /// Retrieves and traces a transaction in its containing block with a custom inspector.
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
                TxResultWithStateFor<Self::Evm>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Insp: evm2::Inspector<EvmTypesFor<Self::Evm>> + Send + 'static,
        R: Send + 'static,
    {
        async move {
            let (transaction, block) = match self.transaction_and_block(hash).await? {
                None => return Ok(None),
                Some(res) => res,
            };
            let (target_tx, tx_info) = transaction.split();
            let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

            self.spawn_with_state_at_block(block.parent_hash(), move |this, mut db| {
                this.apply_pre_execution_changes(&block, &mut db)?;

                Call::replay_transactions_until(
                    &this,
                    &mut db,
                    evm_env.clone(),
                    block.transactions_recovered(),
                    *target_tx.tx_hash(),
                )?;

                let tx_env = this.evm_config().tx_env(target_tx);
                let result = this.inspect(&mut db, evm_env, &tx_env, &mut inspector)?;
                f(tx_info, inspector, result, db)
            })
            .await
            .map(Some)
        }
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
        F: Fn(
                TransactionInfo,
                TracingCtx<'_, Recovered<&ProviderTx<Self::Provider>>, Insp, EvmTypesFor<Self::Evm>>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: Clone + evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
        R: Send + 'static,
    {
        async move {
            let block =
                if block.is_some() { block } else { self.recovered_block(block_id).await? };
            let Some(block) = block else { return Ok(None) };
            let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

            if block.body().transactions().is_empty() {
                return Ok(Some(Vec::new()))
            }

            self.spawn_with_state_at_block(block.parent_hash(), move |this, mut db| {
                let block_hash = block.hash();
                let block_number = evm_env.block_env().number.saturating_to();
                let block_timestamp = evm_env.block_env().timestamp.saturating_to();
                let base_fee = Some(evm_env.block_base_fee());

                this.apply_pre_execution_changes(&block, &mut db)?;

                let max_transactions = highest_index
                    .map(|highest| highest as usize + 1)
                    .unwrap_or_else(|| block.body().transaction_count());
                let mut results = Vec::new();
                let mut inspector = inspector_setup();
                let fused_inspector = inspector.clone();
                let mut evm = this
                    .evm_config()
                    .block_executor_factory()
                    .evm_with_database(&mut db, evm_env.clone());

                for (idx, tx) in block.transactions_recovered().take(max_transactions).enumerate() {
                    let tx_env = this.evm_config().tx_env(tx.cloned());
                    let result = evm
                        .transact_with_inspector(&tx_env, &mut inspector)
                        .map_err(Self::Error::from_evm_err)?;
                    let tx_info = TransactionInfo {
                        hash: Some(*tx.tx_hash()),
                        index: Some(idx as u64),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        block_timestamp: Some(block_timestamp),
                        base_fee,
                    };
                    let evm2::TxResultWithState { result, pending_state, .. } = result;
                    let mut was_fused = false;
                    let item = f(
                        tx_info,
                        TracingCtx {
                            tx,
                            result,
                            state: &pending_state,
                            db: evm.accepted_db_mut(),
                            inspector: &mut inspector,
                            fused_inspector: &fused_inspector,
                            was_fused: &mut was_fused,
                        },
                    )?;
                    if !was_fused {
                        inspector.clone_from(&fused_inspector);
                    }
                    evm.commit_state(&pending_state);
                    results.push(item);
                }

                Ok(Some(results))
            })
            .await
        }
    }

    /// Executes all transactions of a block up to a given index.
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
                    TracingInspector,
                    EvmTypesFor<Self::Evm>,
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
        F: Fn(
                TransactionInfo,
                TracingCtx<
                    '_,
                    Recovered<&ProviderTx<Self::Provider>>,
                    TracingInspector,
                    EvmTypesFor<Self::Evm>,
                >,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
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
        F: Fn(
                TransactionInfo,
                TracingCtx<'_, Recovered<&ProviderTx<Self::Provider>>, Insp, EvmTypesFor<Self::Evm>>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: Clone + evm2::Inspector<EvmTypesFor<Self::Evm>> + 'static,
        R: Send + 'static,
    {
        self.trace_block_until_with_inspector(block_id, block, None, insp_setup, f)
    }

    /// Applies chain-specific state transitions required before executing a block.
    fn apply_pre_execution_changes(
        &self,
        block: &RecoveredBlock<ProviderBlock<Self::Provider>>,
        db: &mut StateCacheDb,
    ) -> Result<evm2::evm::BlockStateAccumulator, Self::Error> {
        let mut executor = self
            .evm_config()
            .executor_for_block(&mut *db, block.sealed_block())
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;
        executor.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;
        let state = executor.execution_state();
        drop(executor);
        db.commit_source(&state);
        Ok(state)
    }
}
