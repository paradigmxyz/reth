use super::BranchNodeCompact;
use reth_codecs::{main_codec, Compact};

/// TODO:
#[main_codec]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StoredSubNode {
    /// The key of the current node.
    pub key: Vec<u8>,
    /// The index of the next child to visit.
    pub nibble: Option<u8>,
    /// The node itself.
    pub node: Option<BranchNodeCompact>,
}
