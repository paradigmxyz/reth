//! Optimism block primitive.

use derive_more::Deref;
use reth_node_api::Block;
use reth_primitives::{BlockBody, Header};

/// An Optimism block.
#[derive(Debug, Deref)]
pub struct OpBlock(reth_primitives::Block);
