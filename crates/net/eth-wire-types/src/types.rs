//! Types and traits for networking types abstraction

use alloy_rlp::{Decodable, Encodable};
use std::fmt::Debug;

/// A trait defining types used in networking.
pub trait NetworkTypes: Send + Sync + Debug + Clone + Unpin + 'static {
    /// The block type. Expected to hold header and body.
    type Block: Encodable + Decodable + Debug + PartialEq + Eq + Clone;
    /// The block body type.
    type BlockBody: Encodable + Decodable + Debug;
    /// The block header type.
    type BlockHeader: Encodable + Decodable + Debug;
}

/// Default networking types
#[derive(Debug, Clone)]
pub struct PrimitiveNetworkTypes;

impl NetworkTypes for PrimitiveNetworkTypes {
    type Block = reth_primitives::Block;
    type BlockBody = reth_primitives::BlockBody;
    type BlockHeader = reth_primitives::Header;
}
