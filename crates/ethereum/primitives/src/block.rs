//! L1 ethereum block primitive.

use derive_more::Deref;
use reth_node_api::Block;
use reth_primitives::{BlockBody, Header};

/// An L1 Ethereum block.
// todo: move reth_primitives type here.
#[derive(Debug, Deref)]
pub struct EthBlock(reth_primitives::Block);
