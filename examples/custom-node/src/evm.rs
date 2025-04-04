use crate::{
    chainspec::CustomChainSpec,
    primitives::{Block, CustomHeader, CustomNodePrimitives},
};
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, ExecutableTx, OnStateHook,
    },
    Database, Evm, EvmEnv,
};
use alloy_op_evm::{OpBlockExecutionCtx, OpBlockExecutor, OpEvm};
use op_revm::{OpSpecId, OpTransaction};
use reth_evm::InspectorFor;
use reth_node_api::ConfigureEvm;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_node::{
    OpBlockAssembler, OpEvmConfig, OpEvmFactory, OpNextBlockEnvAttributes, OpRethReceiptBuilder,
};
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use revm::{
    context::{result::ExecutionResult, TxEnv},
    database::State,
};
use std::sync::Arc;

pub struct CustomBlockExecutor<Evm> {
    inner: OpBlockExecutor<Evm, OpRethReceiptBuilder, Arc<OpChainSpec>>,
}

impl<'db, DB, E> BlockExecutor for CustomBlockExecutor<E>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>, Tx = OpTransaction<TxEnv>>,
{
    type Transaction = OpTransactionSigned;
    type Receipt = OpReceipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>),
    ) -> Result<u64, BlockExecutionError> {
        self.inner.execute_transaction_with_result_closure(tx, f)
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
}

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: OpEvmConfig,
}

impl BlockExecutorFactory for CustomEvmConfig {
    type EvmFactory = OpEvmFactory;
    type ExecutionCtx<'a> = OpBlockExecutionCtx;
    type Transaction = OpTransactionSigned;
    type Receipt = OpReceipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: OpEvm<&'a mut State<DB>, I>,
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

impl ConfigureEvm for CustomEvmConfig {
    type Primitives = CustomNodePrimitives;
    type Error = <OpEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <OpEvmConfig as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory = Self;
    type BlockAssembler = OpBlockAssembler<CustomChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn evm_env(&self, header: &CustomHeader) -> EvmEnv<OpSpecId> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &CustomHeader,
        attributes: &OpNextBlockEnvAttributes,
    ) -> Result<EvmEnv<OpSpecId>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(&self, block: &'a SealedBlock<Block>) -> OpBlockExecutionCtx {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<CustomHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> OpBlockExecutionCtx {
        self.inner.context_for_next_block(parent, attributes)
    }
}
