use crate::primitives::WormholeTransactionSigned;
use reth_primitives_traits::SealedBlock;

/// Wormhole block type using standard header and wormhole transactions
pub type WormholeBlock = alloy_consensus::Block<WormholeTransactionSigned>;

/// Wormhole block body type
pub type WormholeBlockBody = alloy_consensus::BlockBody<WormholeTransactionSigned>;

/// Wormhole block with senders
/// Using the reth-primitives-traits types
pub type WormholeBlockWithSenders = reth_primitives_traits::RecoveredBlock<WormholeBlock>;

/// Wormhole sealed block
pub type WormholeSealedBlock = SealedBlock<WormholeBlock>;

/// Wormhole sealed block with senders
pub type WormholeSealedBlockWithSenders = reth_primitives_traits::RecoveredBlock<WormholeBlock>;

/// Wormhole receipt type (reusing Optimism receipt)
pub type WormholeReceipt = reth_op::OpReceipt;
