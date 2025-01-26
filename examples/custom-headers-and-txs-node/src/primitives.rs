use reth_primitives_traits::NodePrimitives;
use alloy_consensus::Block; 
use crate::header::CustomHeader;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomNodePrimitives;

impl NodePrimitives for CustomNodePrimitives {
    type BlockHeader = CustomHeader;
    type Block = Block<reth_primitives::TransactionSigned, CustomHeader>;
    type BlockBody = alloy_consensus::BlockBody<reth_primitives::TransactionSigned>;
    type SignedTx = reth_primitives::TransactionSigned;
    type Receipt = reth_primitives::Receipt;
}