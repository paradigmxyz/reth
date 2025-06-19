use crate::OpPrimitives;

impl reth_primitives_traits::RpcBlockConversion for OpPrimitives {
    type RpcBlock = alloy_rpc_types_eth::Block<op_alloy_consensus::OpTxEnvelope>;

    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> Option<Self::Block> {
        // For Optimism, we can directly convert to consensus format
        Some(rpc_block.into_consensus())
    }
}
