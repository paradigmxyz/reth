use crate::{
    evm::{
        alloy::{CustomEvm, CustomEvmFactory},
        CustomEvmConfig, CustomTxEnv,
    },
    primitives::CustomTransaction,
};
use alloy_consensus::transaction::Recovered;
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, CommitChanges, ExecutableTx, OnStateHook,
    },
    precompiles::PrecompilesMap,
    Database, Evm,
};
use alloy_op_evm::{OpBlockExecutionCtx, OpBlockExecutor};
use reth_ethereum::evm::primitives::InspectorFor;
use reth_op::{chainspec::OpChainSpec, node::OpRethReceiptBuilder, OpReceipt};
use revm::{context::result::ExecutionResult, database::State};
use std::sync::Arc;

pub struct CustomBlockExecutor<Evm> {
    inner: OpBlockExecutor<Evm, OpRethReceiptBuilder, Arc<OpChainSpec>>,
}

impl<'db, DB, E> BlockExecutor for CustomBlockExecutor<E>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>, Tx = CustomTxEnv>,
{
    type Transaction = CustomTransaction;
    type Receipt = OpReceipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>) -> CommitChanges,
    ) -> Result<Option<u64>, BlockExecutionError> {
        match tx.tx() {
            CustomTransaction::BuiltIn(op_tx) => {
                self.inner.execute_transaction_with_commit_condition(
                    Recovered::new_unchecked(op_tx, *tx.signer()),
                    f,
                )
            }
            CustomTransaction::Other(..) => todo!(),
        }
    }

    fn finish(self) -> Result<(Self::Evm, BlockExecutionResult<OpReceipt>), BlockExecutionError> {
        self.inner.finish()
    }

    fn set_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(_hook)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }
}

impl BlockExecutorFactory for CustomEvmConfig {
    type EvmFactory = CustomEvmFactory;
    type ExecutionCtx<'a> = OpBlockExecutionCtx;
    type Transaction = CustomTransaction;
    type Receipt = OpReceipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.custom_evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: CustomEvm<&'a mut State<DB>, I, PrecompilesMap>,
        ctx: OpBlockExecutionCtx,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        CustomBlockExecutor {
            inner: OpBlockExecutor::new(
                evm,
                ctx,
                self.inner.chain_spec().clone(),
                *self.inner.executor_factory.receipt_builder(),
            ),
        }
    }
}
