use alloy_primitives::B256;
use reth_execution_types::BlockExecutionResult;
use reth_primitives::SealedHeader;
use reth_primitives_traits::NodePrimitives;

/// Input for building a block header.
pub struct BuildHeaderInput<'a, N: NodePrimitives, Attributes> {
    /// Header of the parent block.
    pub parent_header: &'a SealedHeader<N::BlockHeader>,
    /// Body of the current block the header should correspond to.
    pub body: &'a N::BlockBody,
    /// Result of block execution.
    pub execution_result: &'a BlockExecutionResult<N::Receipt>,
    /// Payload attributes.
    pub payload_attributes: &'a Attributes,
    /// Computed state root.
    pub state_root: B256,
}

/// A type that knows how to build a block header.
pub trait HeaderBuilder<Attributes> {
    /// Primitive types of the node.
    type Primitives: NodePrimitives;

    /// Errors that may occur during header building.
    type Error;

    /// Builds a block header.
    fn build_header(
        &self,
        input: BuildHeaderInput<'_, Self::Primitives, Attributes>,
    ) -> Result<<Self::Primitives as NodePrimitives>::BlockHeader, Self::Error>;
}
