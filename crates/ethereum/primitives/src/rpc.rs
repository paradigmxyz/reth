use crate::{EthPrimitives, TransactionSigned};

impl reth_primitives_traits::RpcBlockConversion for EthPrimitives {
    type RpcBlock = alloy_rpc_types_eth::Block;

    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> Self::Block {
        let consensus_block = rpc_block.into_consensus();

        // Convert transactions to the correct type
        consensus_block.convert_transactions::<TransactionSigned>()
    }
}
