use alloy_evm::block::{BlockExecutionError, BlockExecutorFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use reth_op::{
    chainspec::OpChainSpec,
    evm::primitives::execute::{BlockAssembler, BlockAssemblerInput},
    node::OpBlockAssembler,
    DepositReceipt,
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct WormholeBlockAssembler {
    block_assembler: OpBlockAssembler<OpChainSpec>,
}

impl WormholeBlockAssembler {
    pub const fn new(chain_spec: Arc<OpChainSpec>) -> Self {
        Self { block_assembler: OpBlockAssembler::new(chain_spec) }
    }
}

impl<F> BlockAssembler<F> for WormholeBlockAssembler
where
    F: for<'a> BlockExecutorFactory<
        ExecutionCtx<'a> = OpBlockExecutionCtx,
        Transaction = op_alloy_consensus::OpTxEnvelope,
        Receipt: reth_primitives_traits::Receipt + DepositReceipt,
    >,
{
    type Block = alloy_consensus::Block<op_alloy_consensus::OpTxEnvelope>;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F, alloy_consensus::Header>,
    ) -> Result<Self::Block, BlockExecutionError> {
        // For demo purposes, we'll delegate to the inner OP assembler
        // In production, you would handle Wormhole-specific block assembly

        // For now, just create a basic block with the parent header
        // A real implementation would compute the proper header
        Ok(alloy_consensus::Block {
            header: input.parent.clone().unseal(),
            body: reth_op::OpBlockBody {
                transactions: Vec::new(),
                ommers: Vec::new(),
                withdrawals: None,
            },
        })
    }
}
