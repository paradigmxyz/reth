//! Native Compact codec impl for EIP-7685 requests.

use crate::Compact;
use alloc::vec::Vec;
use alloy_primitives::B256;
use alloy_trie::{hash_builder::HashBuilderValue, BranchNodeCompact, TrieMask};
use bytes::{Buf, BufMut};

/// Identifier for [`HashBuilderValue::Hash`]
const HASH_BUILDER_TYPE_HASH: u8 = 0;

/// Identifier for [`HashBuilderValue::Bytes`]
const HASH_BUILDER_TYPE_BYTES: u8 = 1;

impl Compact for HashBuilderValue {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        match self {
            Self::Hash(hash) => {
                buf.put_u8(HASH_BUILDER_TYPE_HASH);
                1 + hash.to_compact(buf)
            }
            Self::Bytes(bytes) => {
                buf.put_u8(HASH_BUILDER_TYPE_BYTES);
                1 + bytes.to_compact(buf)
            }
        }
    }

    // # Panics
    //
    // A panic will be triggered if a HashBuilderValue variant greater than 1 is passed from the
    // database.
    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        match buf.get_u8() {
            HASH_BUILDER_TYPE_HASH => {
                let (hash, buf) = B256::from_compact(buf, 32);
                (Self::Hash(hash), buf)
            }
            HASH_BUILDER_TYPE_BYTES => {
                let (bytes, buf) = Vec::from_compact(buf, 0);
                (Self::Bytes(bytes), buf)
            }
            _ => unreachable!("Junk data in database: unknown HashBuilderValue variant"),
        }
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

        if let Some(root_hash) = self.root_hash {
            buf_size += B256::len_bytes();
            buf.put_slice(root_hash.as_slice());
        }

        for hash in &self.hashes {
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
        let (state_mask, buf) = TrieMask::from_compact(buf, 0);
        let (tree_mask, buf) = TrieMask::from_compact(buf, 0);
        let (hash_mask, buf) = TrieMask::from_compact(buf, 0);

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
        let compact_len = n.to_compact(&mut out);
        assert_eq!(BranchNodeCompact::from_compact(&out, compact_len).0, n);
    }
}
