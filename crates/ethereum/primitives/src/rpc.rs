use crate::EthPrimitives;

impl reth_primitives_traits::RpcBlockConversion for EthPrimitives {
    type RpcBlock = alloy_rpc_types_eth::Block;

    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> Self::Block {
        rpc_block.into_consensus().convert_transactions()
    }
}
