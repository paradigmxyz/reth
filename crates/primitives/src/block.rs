use reth_ethereum_primitives::TransactionSigned;
#[cfg(any(test, feature = "arbitrary"))]
pub use reth_primitives_traits::test_utils::{generate_valid_header, valid_header_strategy};

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type Block<T = TransactionSigned> = alloy_consensus::Block<T>;

/// A response to `GetBlockBodies`, containing bodies if any bodies were found.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type BlockBody<T = TransactionSigned> = alloy_consensus::BlockBody<T>;

/// Ethereum sealed block type
pub type SealedBlock<B = Block> = reth_primitives_traits::block::SealedBlock<B>;

/// Helper type for constructing the block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type SealedBlockFor<B = Block> = reth_primitives_traits::block::SealedBlock<B>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type BlockWithSenders<B = Block> = reth_primitives_traits::block::RecoveredBlock<B>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type SealedBlockWithSenders<B = Block> = reth_primitives_traits::block::RecoveredBlock<B>;
