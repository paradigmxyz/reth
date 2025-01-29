use reth_ethereum_primitives::TransactionSigned;
use crate::primitives::CustomHeader;

/// The Block type of this node
pub type Block = alloy_consensus::Block<TransactionSigned, CustomHeader>;

/// The body type of this node
// TODO: https://github.com/alloy-rs/alloy/pull/1964
pub type BlockBody = alloy_consensus::BlockBody<TransactionSigned>;

