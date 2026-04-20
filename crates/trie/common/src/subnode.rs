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
            // Tag `2`: length-prefixed payload. Tag `1` is the legacy form whose
            // decoder relied on a heuristic over the remaining buffer and could
            // over-read when `state_mask` collided with a plausible payload length.
            // New writes always emit tag `2`; reads still accept `1` so checkpoint
            // bytes written by an older binary remain loadable.
            buf.put_u8(2);
            len += 1;

            let mut node_buf = Vec::new();
            let node_len = node.to_compact(&mut node_buf);
            buf.put_u16(node_len as u16);
            buf.put_slice(&node_buf);
            len += 2 + node_len;
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

        // Node tag: `0` None, `1` legacy (no length prefix), `2` length-prefixed.
        // Legacy bytes decode only when the subnode sits at the tail of the
        // buffer, which is the same constraint the old decoder carried.
        let node = match buf.get_u8() {
            0 => None,
            1 => {
                let (node, rest) = BranchNodeCompact::from_compact(buf, 0);
                buf = rest;
                Some(node)
            }
            2 => {
                let node_len = buf.get_u16() as usize;
                let (node, _) = BranchNodeCompact::from_compact(&buf[..node_len], node_len);
                buf.advance(node_len);
                Some(node)
            }
            tag => panic!("invalid StoredSubNode node tag: {tag}"),
        };

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

    // Covers `state_mask` values whose BE u16 encoding collides with a valid
    // `BranchNodeCompact` compact length (6 + 32*N). Without an explicit length
    // prefix, a heuristic decoder over-reads these and corrupts the trailing bytes.
    #[test]
    fn subnode_with_colliding_state_mask_roundtrips_with_trailing_bytes() {
        for mask in [6u16, 38, 70, 550] {
            let subnode = StoredSubNode {
                key: vec![0x01, 0x02],
                nibble: Some(0x03),
                node: Some(BranchNodeCompact {
                    state_mask: TrieMask::new(mask),
                    tree_mask: TrieMask::new(0),
                    hash_mask: TrieMask::new(mask),
                    hashes: vec![B256::ZERO; mask.count_ones() as usize].into(),
                    root_hash: None,
                }),
            };

            let mut encoded = vec![];
            subnode.to_compact(&mut encoded);
            encoded.extend_from_slice(&[0xaa, 0xbb]);

            let (decoded, rest) = StoredSubNode::from_compact(&encoded[..], 0);

            assert_eq!(subnode, decoded, "mask={mask}");
            assert_eq!(rest, &[0xaa, 0xbb], "mask={mask}");
        }
    }

    // Legacy (tag `1`) bytes still decode when the subnode is at the tail of the
    // buffer. This matches the constraint the pre-fix decoder already carried
    // (`BranchNodeCompact::from_compact` asserts `buf.len() % 32 == 6`).
    #[test]
    fn subnode_decodes_legacy_v1_tag() {
        let node = BranchNodeCompact {
            state_mask: TrieMask::new(0x0001),
            tree_mask: TrieMask::new(0),
            hash_mask: TrieMask::new(0x0001),
            hashes: vec![B256::ZERO].into(),
            root_hash: None,
        };

        // Hand-crafted v1 wire format:
        //   [key_len: u16=0][nibble_tag=0][node_tag=1][BranchNodeCompact raw bytes]
        let mut v1 = Vec::new();
        v1.extend_from_slice(&0u16.to_be_bytes());
        v1.push(0);
        v1.push(1);
        node.to_compact(&mut v1);

        let (decoded, rest) = StoredSubNode::from_compact(&v1[..], 0);
        assert!(rest.is_empty());
        assert_eq!(decoded.key, Vec::<u8>::new());
        assert_eq!(decoded.nibble, None);
        assert_eq!(decoded.node, Some(node));
    }
}
