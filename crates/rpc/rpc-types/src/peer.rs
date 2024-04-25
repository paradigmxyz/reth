use alloy_primitives::B512;

/// Alias for a peer identifier
pub type PeerId = B512;

/// Converts a [`[u8; 65]`] to a [`PeerId`].
pub fn pk_to_id(pk_uncompressed_bytes: &[u8; 65]) -> PeerId {
    // strip out the first byte because that should be the SECP256K1_TAG_PUBKEY_UNCOMPRESSED
    // tag returned by libsecp's uncompressed pubkey serialization
    PeerId::from_slice(&pk_uncompressed_bytes[1..])
}
