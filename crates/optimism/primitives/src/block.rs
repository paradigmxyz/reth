//! Optimism block primitive.

use derive_more::Deref;

/// An Optimism block.
#[derive(Debug, Deref)]
pub struct OpBlock(reth_primitives::Block);
