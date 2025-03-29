//! Native Compact codec impl for alloy-trie types.

use crate::Compact;
use alloc::vec::Vec;
use alloy_primitives::B256;
use alloy_trie::{
    hash_builder::{HashBuilderValue, HashBuilderValueRef},
    nodes::RlpNode,
    BranchNodeCompact, TrieMask,
};
use bytes::{Buf, BufMut};

/// Identifier for [`HashBuilderValueRef::Hash`]
const HASH_BUILDER_TYPE_HASH: u8 = 0;

/// Identifier for [`HashBuilderValueRef::Bytes`]
const HASH_BUILDER_TYPE_BYTES: u8 = 1;

impl Compact for HashBuilderValue {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        match self.as_ref() {
            HashBuilderValueRef::Hash(hash) => {
                buf.put_u8(HASH_BUILDER_TYPE_HASH);
                1 + hash.to_vec().to_compact(buf)
            }
            HashBuilderValueRef::Bytes(bytes) => {
                buf.put_u8(HASH_BUILDER_TYPE_BYTES);
                1 + bytes.to_compact(buf)
            }
        }
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let mut this = Self::default();
        let buf = match buf.get_u8() {
            HASH_BUILDER_TYPE_HASH => {
                let (data, buf) = Vec::from_compact(buf, 32);
                this.set_from_ref(HashBuilderValueRef::Hash(
                    RlpNode::from_raw_rlp(&data).expect("rlp node too large"),
                ));
                buf
            }
            HASH_BUILDER_TYPE_BYTES => {
                let (bytes, buf) = Vec::from_compact(buf, 0);
                this.set_bytes_owned(bytes);
                buf
            }
            _ => unreachable!("Junk data in database: unknown HashBuilderValue variant"),
        };
        (this, buf)
    }
}

impl Compact for BranchNodeCompact {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut buf_size = 0;

        buf_size += self.state_mask.to_compact(buf);
        buf_size += self.tree_mask.to_compact(buf);
        buf_size += self.hash_mask.to_compact(buf);

        let num_hashes = self.hashes.len() as u8 + self.root_hash.is_some() as u8;
        buf.put_u8(num_hashes);

        if let Some(root_hash) = self.root_hash {
            buf_size += B256::len_bytes();
            buf.put_slice(root_hash.as_slice());
        }

        for hash in self.hashes.iter() {
            let hash_len = hash.as_slice().len();
            buf_size += 1 + hash_len;
            buf.put_u8(hash_len as u8);
            buf.put_slice(hash.as_slice());
        }

        buf_size
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        // Consume the masks.
        let (state_mask, buf) = TrieMask::from_compact(buf, 0);
        let (tree_mask, buf) = TrieMask::from_compact(buf, 0);
        let (hash_mask, buf) = TrieMask::from_compact(buf, 0);

        let mut buf = buf;
        let mut root_hash = None;

        let mut num_hashes = buf.get_u8() as usize;

        // Check if the root hash is present
        if hash_mask.count_ones() as usize + 1 == num_hashes {
            root_hash = Some(B256::from_slice(&buf[..B256::len_bytes()]));
            buf.advance(B256::len_bytes());
            num_hashes -= 1;
        }

        // Consume all remaining hashes.
        let mut hashes = Vec::<RlpNode>::with_capacity(num_hashes);
        for _ in 0..num_hashes {
            let hash_len = buf.get_u8() as usize;
            hashes.push(RlpNode::from_raw(&buf[..hash_len]).expect("rlp node too large"));
            buf.advance(hash_len);
        }

        (Self::new(state_mask, tree_mask, hash_mask, hashes, root_hash), buf)
    }
}

impl Compact for TrieMask {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_u16(self.get());
        2
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let mask = buf.get_u16();
        (Self::new(mask), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn node_encoding() {
        let node = BranchNodeCompact::new(
            TrieMask::new(0b1111011000000111),
            TrieMask::new(0b0000000000000101),
            TrieMask::new(0b0100000000000100),
            vec![
                RlpNode::from_raw_rlp(&[1; 33]).unwrap(),
                RlpNode::word_rlp(
                    &hex!("cc35c964dda53ba6c0b87798073a9628dbc9cd26b5cce88eb69655a9c609caf1")
                        .into(),
                ),
            ],
            Some(hex!("aaaabbbb0006767767776fffffeee44444000005567645600000000eeddddddd").into()),
        );

        let mut out = Vec::new();
        let compact_len = node.to_compact(&mut out);
        assert_eq!(BranchNodeCompact::from_compact(&out, compact_len).0, node);
    }
}
