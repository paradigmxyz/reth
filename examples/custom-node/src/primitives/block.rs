use crate::primitives::CustomHeader;
use reth_optimism_primitives::OpTransactionSigned;

/// The Block type of this node
pub type Block = alloy_consensus::Block<OpTransactionSigned, CustomHeader>;

/// The body type of this node
pub type BlockBody = alloy_consensus::BlockBody<OpTransactionSigned, CustomHeader>;
