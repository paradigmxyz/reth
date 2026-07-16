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
    execute::BlockExecutorFactory, ConfigureEvm, Database, Evm, EvmEnvFor, EvmTypesFor, TxEnvFor,
    TxResultWithStateFor,
};
use reth_primitives_traits::{BlockBody, RecoveredBlock};
use reth_rpc_eth_types::{EthApiError, StateCacheDb};
use reth_storage_api::ProviderBlock;
use std::sync::Arc;

/// Context passed to per-transaction trace callbacks.
pub struct TracingCtx<'a, Insp, EvmTypes: evm2::EvmTypes> {
    /// Execution result and detached state changes for the transaction.
    pub result: evm2::TxResultWithState<EvmTypes>,
    /// Database pointing to the transaction pre-state.
    pub db: &'a mut StateCacheDb,
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
            let (inspector, result) =
                this.inspect(&mut db, evm_env, &tx_env, TracingInspector::new(config))?;
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
            let (inspector, result) =
                this.inspect(&mut db, evm_env, &tx_env, TracingInspector::new(config))?;
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
                let (inspector, result) = this.inspect(&mut db, evm_env, &tx_env, inspector)?;
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
                TracingCtx<'_, Insp, EvmTypesFor<Self::Evm>>,
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
                let block_number = block.number();
                let block_timestamp = block.timestamp();
                let base_fee = block.base_fee_per_gas();

                this.apply_pre_execution_changes(&block, &mut db)?;

                let max_transactions = highest_index
                    .map(|highest| highest as usize + 1)
                    .unwrap_or_else(|| block.body().transaction_count());
                let mut results = Vec::new();

                for (idx, tx) in block.transactions_recovered().take(max_transactions).enumerate() {
                    let tx_env = this.evm_config().tx_env(tx.cloned());
                    let (inspector, result) =
                        this.inspect(&mut db, evm_env.clone(), &tx_env, inspector_setup())?;
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
                    db.commit_source(&state_changes);
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
                TracingCtx<'_, TracingInspector, EvmTypesFor<Self::Evm>>,
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
                TracingCtx<'_, Insp, EvmTypesFor<Self::Evm>>,
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
    ) -> Result<reth_evm::EvmState, Self::Error> {
        let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;
        let ctx = self
            .evm_config()
            .context_for_block(block.sealed_block())
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;
        let changes = self
            .evm_config()
            .pre_block_state_changes(&mut *db, evm_env, block.number(), ctx)
            .map_err(|err| EthApiError::EvmCustom(err.to_string()))
            .map_err(Self::Error::from_eth_err)?;
        db.commit_source(&changes);
        Ok(changes)
    }
}
