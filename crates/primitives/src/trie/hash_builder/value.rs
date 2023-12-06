use crate::B256;
use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

/// The current value of the hash builder.
#[derive_arbitrary(compact)]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum HashBuilderValue {
    /// Value of the leaf node.
    Hash(B256),
    /// Hash of adjacent nodes.
    Bytes(Vec<u8>),
}

impl Compact for HashBuilderValue {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        match self {
            Self::Hash(hash) => {
                buf.put_u8(0);
                1 + hash.to_compact(buf)
            }
            Self::Bytes(bytes) => {
                buf.put_u8(1);
                1 + bytes.to_compact(buf)
            }
        }
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        match buf[0] {
            0 => {
                let (hash, buf) = B256::from_compact(&buf[1..], 32);
                (Self::Hash(hash), buf)
            }
            1 => {
                let (bytes, buf) = Vec::from_compact(&buf[1..], 0);
                (Self::Bytes(bytes), buf)
            }
            _ => unreachable!("Junk data in database: unknown HashBuilderValue variant"),
        }
    }
}

impl std::fmt::Debug for HashBuilderValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(bytes) => write!(f, "Bytes({:?})", crate::hex::encode(bytes)),
            Self::Hash(hash) => write!(f, "Hash({:?})", hash),
        }
    }
}

impl From<Vec<u8>> for HashBuilderValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<&[u8]> for HashBuilderValue {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.to_vec())
    }
}

impl From<B256> for HashBuilderValue {
    fn from(value: B256) -> Self {
        Self::Hash(value)
    }
}

impl Default for HashBuilderValue {
    fn default() -> Self {
        Self::Bytes(vec![])
    }
}
