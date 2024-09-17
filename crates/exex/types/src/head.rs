use reth_primitives::BlockNumHash;

/// A head of the ExEx. It determines the highest block committed to the internal ExEx state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExExHead {
    /// The head block.
    pub block: BlockNumHash,
}
