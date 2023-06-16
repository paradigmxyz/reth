use super::BranchNodeCompact;
use bytes::Buf;
use reth_codecs::Compact;

/// Walker sub node for storing intermediate state root calculation state in the database.
/// See [crate::stage::MerkleCheckpoint].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StoredSubNode {
    /// The key of the current node.
    pub key: Vec<u8>,
    /// The index of the next child to visit.
    pub nibble: Option<u8>,
    /// The node itself.
    pub node: Option<BranchNodeCompact>,
}

impl Compact for StoredSubNode {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut len = 0;

        buf.put_u16(self.key.len() as u16);
        buf.put_slice(&self.key[..]);
        len += 2 + self.key.len();

        if let Some(nibble) = self.nibble {
            buf.put_u8(1);
            buf.put_u8(nibble);
            len += 2;
        } else {
            buf.put_u8(0);
            len += 1;
        }

        if let Some(node) = self.node {
            buf.put_u8(1);
            len += 1;
            len += node.to_compact(buf);
        } else {
            len += 1;
            buf.put_u8(0);
        }

        len
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        let key_len = buf.get_u16() as usize;
        let key = Vec::from(&buf[..key_len]);
        buf.advance(key_len);

        let nibbles_exists = buf.get_u8() != 0;
        let nibble = if nibbles_exists { Some(buf.get_u8()) } else { None };

        let node_exsists = buf.get_u8() != 0;
        let node = if node_exsists {
            let (node, rest) = BranchNodeCompact::from_compact(buf, 0);
            buf = rest;
            Some(node)
        } else {
            None
        };

        (StoredSubNode { key, nibble, node }, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{trie::TrieMask, H256};

    #[test]
    fn subnode_roundtrip() {
        let subnode = StoredSubNode {
            key: vec![],
            nibble: None,
            node: Some(BranchNodeCompact {
                state_mask: TrieMask::new(1),
                tree_mask: TrieMask::new(0),
                hash_mask: TrieMask::new(1),
                hashes: vec![H256::zero()],
                root_hash: None,
            }),
        };

        let mut encoded = vec![];
        subnode.clone().to_compact(&mut encoded);
        let (decoded, _) = StoredSubNode::from_compact(&encoded[..], 0);

        assert_eq!(subnode, decoded);
    }
}
