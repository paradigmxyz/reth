use crate::trie::StoredTrieMask;
use alloy_primitives::B256;
use alloy_trie::BranchNodeCompact;
use bytes::Buf;
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

/// Wrapper around `BranchNodeCompact` that implements `Compact`.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBranchNode(pub BranchNodeCompact);

impl Compact for StoredBranchNode {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let BranchNodeCompact { state_mask, tree_mask, hash_mask, root_hash, hashes } = self.0;

        let mut buf_size = 0;

        buf_size += StoredTrieMask(state_mask).to_compact(buf);
        buf_size += StoredTrieMask(tree_mask).to_compact(buf);
        buf_size += StoredTrieMask(hash_mask).to_compact(buf);

        if let Some(root_hash) = root_hash {
            buf_size += B256::len_bytes();
            buf.put_slice(root_hash.as_slice());
        }

        for hash in &hashes {
            buf_size += B256::len_bytes();
            buf.put_slice(hash.as_slice());
        }

        buf_size
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let hash_len = B256::len_bytes();

        // Assert the buffer is long enough to contain the masks and the hashes.
        assert_eq!(buf.len() % hash_len, 6);

        // Consume the masks.
        let (StoredTrieMask(state_mask), buf) = StoredTrieMask::from_compact(buf, 0);
        let (StoredTrieMask(tree_mask), buf) = StoredTrieMask::from_compact(buf, 0);
        let (StoredTrieMask(hash_mask), buf) = StoredTrieMask::from_compact(buf, 0);

        let mut buf = buf;
        let mut num_hashes = buf.len() / hash_len;
        let mut root_hash = None;

        // Check if the root hash is present
        if hash_mask.count_ones() as usize + 1 == num_hashes {
            root_hash = Some(B256::from_slice(&buf[..hash_len]));
            buf.advance(hash_len);
            num_hashes -= 1;
        }

        // Consume all remaining hashes.
        let mut hashes = Vec::<B256>::with_capacity(num_hashes);
        for _ in 0..num_hashes {
            hashes.push(B256::from_slice(&buf[..hash_len]));
            buf.advance(hash_len);
        }

        (Self(BranchNodeCompact::new(state_mask, tree_mask, hash_mask, hashes, root_hash)), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn node_encoding() {
        let n = BranchNodeCompact::new(
            0xf607,
            0x0005,
            0x4004,
            vec![
                hex!("90d53cd810cc5d4243766cd4451e7b9d14b736a1148b26b3baac7617f617d321").into(),
                hex!("cc35c964dda53ba6c0b87798073a9628dbc9cd26b5cce88eb69655a9c609caf1").into(),
            ],
            Some(hex!("aaaabbbb0006767767776fffffeee44444000005567645600000000eeddddddd").into()),
        );

        let mut out = Vec::new();
        let compact_len = StoredBranchNode(n.clone()).to_compact(&mut out);
        assert_eq!(StoredBranchNode::from_compact(&out, compact_len).0 .0, n);
    }
}
