//! L1 ethereum block primitive.

use derive_more::Deref;

/// An L1 Ethereum block.
// todo: move reth_primitives type here.
#[derive(Debug, Deref)]
pub struct EthBlock(reth_primitives::Block);
