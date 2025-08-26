use alloy_consensus::{Block as AlloyBlock, Header};
use reth_ethereum_primitives::TransactionSigned;
#[cfg(any(test, feature = "arbitrary"))]
pub use reth_primitives_traits::test_utils::{generate_valid_header, valid_header_strategy};

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type Block = AlloyBlock<TransactionSigned>;

/// A response to `GetBlockBodies`, containing bodies if any bodies were found.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type BlockBody = alloy_consensus::BlockBody<TransactionSigned>;

/// Ethereum sealed block type
pub type SealedBlock = reth_primitives_traits::block::SealedBlock<Block>;

/// Helper type for constructing the block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type SealedBlockFor = reth_primitives_traits::block::SealedBlock<Block>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type BlockWithSenders = reth_primitives_traits::block::RecoveredBlock<Block>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type SealedBlockWithSenders = reth_primitives_traits::block::RecoveredBlock<Block>;

#[cfg(test)]
mod test_debug;
