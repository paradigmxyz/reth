use crate::{
    pool::CustomPooledTransaction,
    primitives::{CustomHeader, CustomTransaction},
};
use alloy_consensus::{Block, BlockBody};
use reth_ethereum::network::NetworkPrimitives;
use reth_op::OpReceipt;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CustomNetworkPrimitives;

impl NetworkPrimitives for CustomNetworkPrimitives {
    type BlockHeader = CustomHeader;
    type BlockBody = BlockBody<CustomTransaction, CustomHeader>;
    type Block = Block<CustomTransaction, CustomHeader>;
    type BroadcastedTransaction = CustomTransaction;
    type PooledTransaction = CustomPooledTransaction;
    type Receipt = OpReceipt;
}
