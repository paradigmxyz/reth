//! Error types that can occur in this crate.

/// Error thrown when decoding a UDP packet.
#[derive(Debug, thiserror::Error)]
pub enum DecodePacketError {
    #[error("Failed to rlp decode: {0:?}")]
    Rlp(#[from] reth_rlp::DecodeError),
    #[error("Received packet len too short.")]
    PacketTooShort,
    #[error("Hash of the header not equals to the hash of the data.")]
    HashMismatch,
    #[error("Message id {0} is not supported.")]
    UnknownMessage(u8),
    #[error("Failed to recover public key: {0:?}")]
    Secp256k1(#[from] secp256k1::Error),
}
