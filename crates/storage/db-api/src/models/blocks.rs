//! Block related models and types.

use alloy_consensus::Header;
use alloy_primitives::B256;
use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// The storage representation of a block's ommers.
///
/// It is stored as the headers of the block's uncles.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StoredBlockOmmers<H = Header> {
    /// The block headers of this block's uncles.
    pub ommers: Vec<H>,
}

impl<H: Compact> Compact for StoredBlockOmmers<H> {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut buffer = bytes::BytesMut::new();
        self.ommers.to_compact(&mut buffer);
        let total_length = buffer.len();
        buf.put(buffer);
        total_length
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let (ommers, new_buf) = Vec::from_compact(buf, buf.len());
        (Self { ommers }, new_buf)
    }
}

/// Hash of the block header.
pub type HeaderHash = B256;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Compress, Decompress};

    #[test]
    fn test_ommer() {
        let mut ommer = StoredBlockOmmers::default();
        ommer.ommers.push(Header::default());
        ommer.ommers.push(Header::default());
        assert_eq!(ommer.clone(), StoredBlockOmmers::decompress(&ommer.compress()).unwrap());
    }

    #[test]
    fn fuzz_stored_block_ommers() {
        fuzz_test_stored_block_ommers(StoredBlockOmmers::default())
    }

    #[test_fuzz::test_fuzz]
    fn fuzz_test_stored_block_ommers(obj: StoredBlockOmmers) {
        use reth_codecs::Compact;
        let mut buf = vec![];
        let len = obj.to_compact(&mut buf);
        let (same_obj, _) = StoredBlockOmmers::from_compact(buf.as_ref(), len);
        assert_eq!(obj, same_obj);
    }
}
