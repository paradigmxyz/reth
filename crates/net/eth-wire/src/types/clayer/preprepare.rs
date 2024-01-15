//! Implementation of consensus layer messages[PrePrepare]
use alloy_rlp::{Encodable, RlpDecodable, RlpEncodable};
use reth_codecs::derive_arbitrary;
use reth_primitives::{keccak256, Bytes, B256};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Consensus layer message[Prepare]
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PrePrepare {
    /// pbft view number
    pub view: u64,
    /// pbft sequence number
    pub sequence: u64,
    /// request digest
    pub digest: B256,
    /// request
    pub request: Bytes,
}

impl PrePrepare {
    /// Create new message[PrePrepare]
    pub fn new(view: u64, sequence: u64, request: Bytes) -> Self {
        let digest = keccak256(&request);
        Self { view, sequence, digest, request }
    }

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
