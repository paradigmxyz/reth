use crate::{
    chainspec::CustomChainSpec,
    primitives::{block::Block, tx, CustomHeader, CustomNodePrimitives, CustomTransaction},
};
use alloy_consensus::Header;
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, ExecutableTx, OnStateHook,
    },
    precompiles::PrecompilesMap,
    Database, Evm, EvmEnv,
};
use alloy_op_evm::{
    block::receipt_builder::OpReceiptBuilder, OpBlockExecutionCtx, OpBlockExecutor, OpEvm,
};
use op_revm::{OpSpecId, OpTransaction};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_ethereum::{
    evm::primitives::{
        execute::{BlockAssembler, BlockAssemblerInput},
        InspectorFor,
    },
    node::api::ConfigureEvm,
    primitives::{Receipt, SealedBlock, SealedHeader},
};
use reth_op::{
    chainspec::OpChainSpec,
    node::{
        OpBlockAssembler, OpEvmConfig, OpEvmFactory, OpNextBlockEnvAttributes, OpRethReceiptBuilder,
    },
    DepositReceipt, OpReceipt, OpTransactionSigned,
};
use reth_primitives_traits::NodePrimitives;
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
    type Transaction = CustomTransaction;
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

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }
}

#[derive(Clone, Debug)]
pub struct CustomBlockAssembler {
    inner: OpBlockAssembler<CustomChainSpec>,
}

impl CustomBlockAssembler {
    pub fn new(inner: OpBlockAssembler<CustomChainSpec>) -> Self {
        Self { inner }
    }
}

impl<F> BlockAssembler<F> for CustomBlockAssembler
where
    F: for<'a> BlockExecutorFactory<
        ExecutionCtx<'a> = OpBlockExecutionCtx,
        Transaction = OpTransactionSigned,
        Receipt: Receipt + DepositReceipt,
    >,
{
    type Block = Block;

    fn assemble_block(
        &self,
        mut input: BlockAssemblerInput<'_, '_, F, CustomHeader>,
    ) -> Result<Self::Block, BlockExecutionError> {
        let inner_input = BlockAssemblerInput {
            evm_env: input.evm_env,
            execution_ctx: input.execution_ctx,
            parent: &input.parent.inner,
            transactions: input.transactions,
            output: input.output,
            bundle_state: input.bundle_state,
            state_provider: input.state_provider,
            state_root: input.state_root,
        };
        let block = self.inner.assemble_block(inner_input)?;
        let block = block.map_transactions(tx::from);
        let block = block.map_header(CustomHeader::from);

        Ok(block)
    }
}

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: OpEvmConfig,
    block_assembler: CustomBlockAssembler,
}

impl CustomEvmConfig {
    pub fn new(inner: OpEvmConfig, block_assembler: CustomBlockAssembler) -> Self {
        Self { inner, block_assembler }
    }
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
        evm: OpEvm<&'a mut State<DB>, I, PrecompilesMap>,
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
    type BlockAssembler = CustomBlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &Header) -> EvmEnv<OpSpecId> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &OpNextBlockEnvAttributes,
    ) -> Result<EvmEnv<OpSpecId>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block(
        &self,
        block: &SealedBlock<alloy_consensus::Block<OpTransactionSigned>>,
    ) -> OpBlockExecutionCtx {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> OpBlockExecutionCtx {
        self.inner.context_for_next_block(parent, attributes)
    }
}
