//! Native Compact codec impl for alloy-trie types.

use crate::Compact;
use alloc::vec::Vec;
use alloy_primitives::B256;
use alloy_trie::{
    hash_builder::{HashBuilderValue, HashBuilderValueRef},
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
                1 + hash.to_compact(buf)
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
                let (hash, buf) = B256::from_compact(buf, 32);
                this.set_from_ref(HashBuilderValueRef::Hash(&hash));
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

        if let Some(root_hash) = self.root_hash {
            buf_size += B256::len_bytes();
            buf.put_slice(root_hash.as_slice());
        }

        for hash in self.hashes.iter() {
            buf_size += B256::len_bytes();
            buf.put_slice(hash.as_slice());
        }

        buf_size
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let hash_len = B256::len_bytes();

        // Use only the specified length from the buffer, not the entire remaining buffer
        let buf_to_decode = if len > 0 { &buf[..len] } else { buf };
        
        // Assert the buffer is long enough to contain the masks and the hashes.
        assert_eq!(buf_to_decode.len() % hash_len, 6);

        // Consume the masks from the original buffer
        let (state_mask, buf_after_masks) = TrieMask::from_compact(buf, 0);
        let (tree_mask, buf_after_masks) = TrieMask::from_compact(buf_after_masks, 0);
        let (hash_mask, buf_after_masks) = TrieMask::from_compact(buf_after_masks, 0);

        let mut current_buf = buf_after_masks;
        // Calculate the number of hashes from the buffer we're supposed to consume
        let hashes_bytes = if len > 0 { len - 6 } else { current_buf.len() };
        let mut num_hashes = hashes_bytes / hash_len;
        let mut root_hash = None;

        // Check if the root hash is present
        if hash_mask.count_ones() as usize + 1 == num_hashes {
            root_hash = Some(B256::from_slice(&current_buf[..hash_len]));
            current_buf.advance(hash_len);
            num_hashes -= 1;
        }

        // Consume all remaining hashes.
        let mut hashes = Vec::<B256>::with_capacity(num_hashes);
        for _ in 0..num_hashes {
            hashes.push(B256::from_slice(&current_buf[..hash_len]));
            current_buf.advance(hash_len);
        }

        // Return the remaining buffer after consuming exactly `len` bytes
        let bytes_consumed = if len > 0 { len } else { buf.len() - current_buf.len() };
        (Self::new(state_mask, tree_mask, hash_mask, hashes, root_hash), &buf[bytes_consumed..])
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
