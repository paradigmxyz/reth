use crate::{
    chainspec::CustomChainSpec,
    primitives::{CustomHeader, CustomNodePrimitives},
};
use alloy_evm::block::BlockExecutorFor;
use alloy_op_evm::{OpBlockExecutionCtx, OpBlockExecutor, OpEvm, OpEvmFactory};
use op_revm::OpSpecId;
use reth_evm::{execute::BlockExecutorFactory, ConfigureEvm, Database, EvmEnv, InspectorFor};
use reth_optimism_node::{OpBlockAssembler, OpEvmConfig, OpNextBlockEnvAttributes};
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use reth_primitives::SealedBlock;
use reth_primitives_traits::SealedHeader;
use reth_revm::State;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: OpEvmConfig<CustomChainSpec>,
}

impl CustomEvmConfig {
    pub fn new(chain_spec: Arc<CustomChainSpec>) -> Self {
        Self { inner: OpEvmConfig::optimism(chain_spec) }
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
                self.inner.chain_spec(),
                self.inner.executor_factory.receipt_builder(),
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

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<OpTransactionSigned>,
    ) -> OpBlockExecutionCtx {
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
