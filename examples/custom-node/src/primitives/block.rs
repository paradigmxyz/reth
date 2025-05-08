use crate::primitives::{CustomHeader, CustomTransactionEnvelope};
use reth_op::primitives::ExtendedOpTxEnvelope;

/// The Block type of this node
pub type Block =
    alloy_consensus::Block<ExtendedOpTxEnvelope<CustomTransactionEnvelope>, CustomHeader>;

/// The body type of this node
pub type BlockBody =
    alloy_consensus::BlockBody<ExtendedOpTxEnvelope<CustomTransactionEnvelope>, CustomHeader>;
