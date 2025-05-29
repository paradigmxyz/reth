use crate::primitives::{CustomHeader, CustomTransaction, CustomTransactionEnvelope};
use alloy_consensus::{Block, BlockBody};
use op_alloy_consensus::OpPooledTransaction;
use reth_ethereum::{network::NetworkPrimitives, primitives::Extended};
use reth_op::OpReceipt;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CustomNetworkPrimitives;

impl NetworkPrimitives for CustomNetworkPrimitives {
    type BlockHeader = CustomHeader;
    type BlockBody = BlockBody<CustomTransaction, CustomHeader>;
    type Block = Block<CustomTransaction, CustomHeader>;
    type BroadcastedTransaction = CustomTransaction;
    type PooledTransaction = Extended<OpPooledTransaction, CustomTransactionEnvelope>;
    type Receipt = OpReceipt;
}
