use reth_primitives::{BlockHash, BlockNumber};

#[allow(clippy::doc_markdown)]
/// A head of the ExEx. It should determine the highest block committed to the internal ExEx state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExExHead {
    /// The number of the ExEx head block.
    pub number: BlockNumber,
    /// The hash of the ExEx head block.
    pub hash: BlockHash,
}
