//! RPC conversion traits for node types.

use crate::NodePrimitives;

/// Trait for converting RPC blocks to primitive blocks for a specific node type.
pub trait RpcBlockConversion: NodePrimitives {
    /// The RPC block type.
    type RpcBlock: serde::Serialize + serde::de::DeserializeOwned + 'static;

    /// Converts an RPC block to the node's primitive block type.
    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> Option<Self::Block>;
}
