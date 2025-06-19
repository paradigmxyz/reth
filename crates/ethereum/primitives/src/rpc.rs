use crate::{EthPrimitives, TransactionSigned};

impl reth_primitives_traits::RpcBlockConversion for EthPrimitives {
    type RpcBlock = alloy_rpc_types_eth::Block;

    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> Option<Self::Block> {
        // Convert the RPC block to consensus format
        let consensus_block = rpc_block.into_consensus();

        // Convert transactions to the correct type
        let block = consensus_block.convert_transactions::<TransactionSigned>();

        Some(block)
    }
}
