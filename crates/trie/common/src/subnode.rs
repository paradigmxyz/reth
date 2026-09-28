use super::BranchNodeCompact;
use alloc::vec::Vec;

/// Walker sub node for storing intermediate state root calculation state in the database.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoredSubNode {
    /// The key of the current node.
    pub key: Vec<u8>,
    /// The index of the next child to visit.
    pub nibble: Option<u8>,
    /// The node itself.
    pub node: Option<BranchNodeCompact>,
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StoredSubNode {
    fn to_compact<B>(&self, buf: &mut B) -> usize
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

        if let Some(node) = &self.node {
            // `BranchNodeCompact` infers its number of hashes from the length of the buffer it
            // is decoded from, so prefix it with its length to allow more data to follow it.
            let mut node_buf = Vec::new();
            let node_len = node.to_compact(&mut node_buf);
            buf.put_u8(1);
            buf.put_u16(node_len as u16);
            buf.put_slice(&node_buf);
            len += 3 + node_len;
        } else {
            len += 1;
            buf.put_u8(0);
        }

        len
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        let key_len = buf.get_u16() as usize;
        let key = Vec::from(&buf[..key_len]);
        buf.advance(key_len);

        let nibbles_exists = buf.get_u8() != 0;
        let nibble = nibbles_exists.then(|| buf.get_u8());

        let node_exists = buf.get_u8() != 0;
        let node = node_exists.then(|| {
            let node_len = buf.get_u16() as usize;
            let (node, _) = BranchNodeCompact::from_compact(&buf[..node_len], node_len);
            buf.advance(node_len);
            node
        });

        (Self { key, nibble, node }, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrieMask;
    use alloy_primitives::B256;
    use reth_codecs::Compact;

    #[test]
    fn subnode_roundtrip() {
        let subnode = StoredSubNode {
            key: vec![],
            nibble: None,
            node: Some(BranchNodeCompact {
                state_mask: TrieMask::new(1),
                tree_mask: TrieMask::new(0),
                hash_mask: TrieMask::new(1),
                hashes: vec![B256::ZERO].into(),
                root_hash: None,
            }),
        };

        let mut encoded = vec![];
        subnode.to_compact(&mut encoded);
        let (decoded, _) = StoredSubNode::from_compact(&encoded[..], 0);

        assert_eq!(subnode, decoded);
    }

    #[test]
    fn subnode_with_trailing_data_roundtrip() {
        let trailing = [0xab; 7];
        for root_hash in [None, Some(B256::repeat_byte(0xcc))] {
            let subnode = StoredSubNode {
                key: vec![0x1, 0x2],
                nibble: Some(0x3),
                node: Some(BranchNodeCompact::new(
                    0b1010,
                    0b0010,
                    0b1000,
                    vec![B256::repeat_byte(0xaa)],
                    root_hash,
                )),
            };

            let mut encoded = vec![];
            let len = subnode.to_compact(&mut encoded);
            assert_eq!(len, encoded.len());
            encoded.extend_from_slice(&trailing);
            let (decoded, rest) = StoredSubNode::from_compact(&encoded, len);

            assert_eq!(decoded, subnode);
            assert_eq!(rest, trailing);
        }
    }
}
