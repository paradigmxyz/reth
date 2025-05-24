use crate::{
    chainspec::CustomChainSpec,
    primitives::{Block, CustomHeader, CustomTransaction},
};
use alloy_evm::block::{BlockExecutionError, BlockExecutorFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use reth_ethereum::{
    evm::primitives::execute::{BlockAssembler, BlockAssemblerInput},
    primitives::Receipt,
};
use reth_op::{node::OpBlockAssembler, DepositReceipt};

#[derive(Clone, Debug)]
pub struct CustomBlockAssembler {
    block_assembler: OpBlockAssembler<CustomChainSpec>,
}

impl<F> BlockAssembler<F> for CustomBlockAssembler
where
    F: for<'a> BlockExecutorFactory<
        ExecutionCtx<'a> = OpBlockExecutionCtx,
        Transaction = CustomTransaction,
        Receipt: Receipt + DepositReceipt,
    >,
{
    type Block = Block;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F, CustomHeader>,
    ) -> Result<Self::Block, BlockExecutionError> {
        Ok(self.block_assembler.assemble_block(input)?.map_header(From::from))
    }
}
