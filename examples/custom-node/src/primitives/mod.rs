//! Contains the primitive types of this node.

pub mod header;
pub use header::*;
pub mod block;
pub use block::*;

use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::NodePrimitives;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomNodePrimitives;

impl NodePrimitives for CustomNodePrimitives {
    type Block = Block;
    type BlockHeader = CustomHeader;
    type BlockBody = BlockBody;
    type SignedTx = TransactionSigned;
    type Receipt = reth_ethereum_primitives::Receipt;
}
