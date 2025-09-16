//! Trie related models and types.

use reth_codecs::Compact;
use reth_trie_common::BranchNodeCompact;
use serde::{Deserialize, Serialize};

/// Branch node, or the absence of one, as it is saved in the database.
#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BranchNodeBeforeBlock(Option<BranchNodeCompact>);

impl Compact for BranchNodeBeforeBlock {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self.0.as_ref().map(|node| node.to_compact(buf)).unwrap_or(0)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (Self(None), buf)
        }

        let (node, buf) = BranchNodeCompact::from_compact(buf, len);
        (Self(Some(node)), buf)
    }
}

impl From<Option<BranchNodeCompact>> for BranchNodeBeforeBlock {
    fn from(v: Option<BranchNodeCompact>) -> Self {
        Self(v)
    }
}

impl From<BranchNodeBeforeBlock> for Option<BranchNodeCompact> {
    fn from(v: BranchNodeBeforeBlock) -> Self {
        v.0
    }
}
