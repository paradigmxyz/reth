use alloy_primitives::B512;

/// Alias for a peer identifier
pub type PeerId = B512;

/// Converts a [`secp256k1::PublicKey`] to a [`PeerId`].
pub fn pk_to_id(pk: &secp256k1::PublicKey) -> PeerId {
    PeerId::from_slice(&pk.serialize_uncompressed()[1..])
}
