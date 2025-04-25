//! Contains the primitive types of this node.

pub mod header;
pub use header::*;
pub mod block;
pub use block::*;
pub mod tx;
pub use tx::*;
pub mod extended_op_tx_envelope;
pub use extended_op_tx_envelope::*;

use reth_ethereum::primitives::NodePrimitives;
use reth_op::{OpReceipt, OpTransactionSigned};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomNodePrimitives;

impl NodePrimitives for CustomNodePrimitives {
    type Block = Block;
    type BlockHeader = CustomHeader;
    type BlockBody = BlockBody;
    type SignedTx = OpTransactionSigned;
    type Receipt = OpReceipt;
}
