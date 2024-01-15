//! Implementation of consensus layer messages[Commit]
use alloy_rlp::{Encodable, RlpDecodable, RlpEncodable};
use reth_codecs::derive_arbitrary;
use reth_primitives::{keccak256, Bytes, B256};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
/// Consensus layer message[Commit]
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Commit {
    /// pbft view number
    pub view: u64,
    /// pbft sequence number
    pub sequence: u64,
    /// request digest
    pub digest: B256,
}

impl Commit {
    /// Compute the hash of the message
    pub fn hash_slow(&self) -> B256 {
        let mut out = bytes::BytesMut::new();
        self.encode(&mut out);
        keccak256(&out)
    }

    /// Convert to bytes
    pub fn as_bytes(&self) -> reth_primitives::Bytes {
        let mut out = bytes::BytesMut::new();
        self.encode(&mut out);
        Bytes::from(bytes::Bytes::from(out))
    }
}
