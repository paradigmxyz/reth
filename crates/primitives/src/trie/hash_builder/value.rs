use alloy_primitives::B256;
use alloy_trie::hash_builder::HashBuilderValue;
use bytes::Buf;
use reth_codecs::Compact;

/// A wrapper around `HashBuilderValue` that implements `Compact`.
pub(crate) struct StoredHashBuilderValue(pub(crate) HashBuilderValue);

impl Compact for StoredHashBuilderValue {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        match self.0 {
            HashBuilderValue::Hash(hash) => {
                buf.put_u8(0);
                1 + hash.to_compact(buf)
            }
            HashBuilderValue::Bytes(bytes) => {
                buf.put_u8(1);
                1 + bytes.to_compact(buf)
            }
        }
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        match buf.get_u8() {
            0 => {
                let (hash, buf) = B256::from_compact(buf, 32);
                (Self(HashBuilderValue::Hash(hash)), buf)
            }
            1 => {
                let (bytes, buf) = Vec::from_compact(buf, 0);
                (Self(HashBuilderValue::Bytes(bytes)), buf)
            }
            _ => unreachable!("Junk data in database: unknown HashBuilderValue variant"),
        }
    }
}
