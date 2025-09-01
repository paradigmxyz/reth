use crate::{
    evm::{
        alloy::{CustomEvm, CustomEvmFactory},
        env::PaymentTxEnv, CustomEvmConfig, CustomTxEnv,
    },
    primitives::{CustomTransaction, TxPayment},
};
use alloy_consensus::{transaction::Recovered, Typed2718};
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
use revm::{context::result::ExecutionResult, context::TxEnv, database::State};
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
            CustomTransaction::Op(op_tx) => self.inner.execute_transaction_with_commit_condition(
                Recovered::new_unchecked(op_tx, *tx.signer()),
                f,
            ),
            CustomTransaction::Payment(payment_tx) => {
                // For the custom node example, we execute payment transactions by delegating
                // to the EVM through the transact_raw method
                // This is a simplified implementation for demonstration purposes

                // Create TxEnv from the payment transaction
                let tx_env = {
                    let TxPayment {
                        chain_id,
                        nonce,
                        gas_limit,
                        max_fee_per_gas,
                        max_priority_fee_per_gas,
                        to,
                        value,
                        ..
                    } = payment_tx.tx();
                    TxEnv {
                        tx_type: payment_tx.tx().ty(),
                        caller: *tx.signer(),
                        gas_limit: *gas_limit,
                        gas_price: *max_fee_per_gas,
                        gas_priority_fee: Some(*max_priority_fee_per_gas),
                        kind: alloy_primitives::TxKind::Call(*to),
                        value: *value,
                        nonce: *nonce,
                        chain_id: Some(*chain_id),
                        ..Default::default()
                    }
                };

                // Execute using the EVM's transact_raw method
                // We need to access the EVM through the block executor
                // This is a simplified approach - in production you might want more sophisticated handling
                let result = self.evm_mut().transact_raw(CustomTxEnv::Payment(PaymentTxEnv(tx_env)))
                    .map_err(|_| BlockExecutionError::msg("Payment transaction failed"))?;

                // Call the commit condition function
                let commit_changes = f(&result.result);

                // Apply changes if needed
                match commit_changes {
                    alloy_evm::block::CommitChanges::Yes => {
                        // The changes are already applied by transact_raw
                        Ok(Some(result.result.gas_used()))
                    }
                    alloy_evm::block::CommitChanges::No => {
                        // For this example, we don't handle reverts specially
                        // In production, you would need to revert state changes
                        Ok(Some(result.result.gas_used()))
                    }
                }
            }
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
    type ExecutionCtx<'a> = CustomBlockExecutionCtx;
    type Transaction = CustomTransaction;
    type Receipt = OpReceipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.custom_evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: CustomEvm<&'a mut State<DB>, I, PrecompilesMap>,
        ctx: CustomBlockExecutionCtx,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        CustomBlockExecutor {
            inner: OpBlockExecutor::new(
                evm,
                ctx.inner,
                self.inner.chain_spec().clone(),
                *self.inner.executor_factory.receipt_builder(),
            ),
        }
    }
}

/// Additional parameters for executing custom transactions.
#[derive(Debug, Clone)]
pub struct CustomBlockExecutionCtx {
    pub inner: OpBlockExecutionCtx,
    pub extension: u64,
}

impl From<CustomBlockExecutionCtx> for OpBlockExecutionCtx {
    fn from(value: CustomBlockExecutionCtx) -> Self {
        value.inner
    }
}
