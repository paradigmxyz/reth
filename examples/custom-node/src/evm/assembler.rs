use crate::chainspec::CustomChainSpec;
use alloy_consensus::Block;
use alloy_evm::block::{BlockExecutionError, BlockExecutorFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use reth_ethereum::{
    evm::primitives::execute::{BlockAssembler, BlockAssemblerInput},
    primitives::Receipt,
};
use reth_op::{node::OpBlockAssembler, DepositReceipt, OpTransactionSigned};

#[derive(Clone, Debug)]
pub struct CustomBlockAssembler {
    inner: OpBlockAssembler<CustomChainSpec>,
}

impl<F> BlockAssembler<F> for CustomBlockAssembler
where
    F: for<'a> BlockExecutorFactory<
        ExecutionCtx<'a> = OpBlockExecutionCtx,
        Transaction = OpTransactionSigned,
        Receipt: Receipt + DepositReceipt,
    >,
{
    // TODO: use custom block here
    type Block = Block<OpTransactionSigned>;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F>,
    ) -> Result<Self::Block, BlockExecutionError> {
        let block = self.inner.assemble_block(input)?;

        Ok(block)
    }
}
