use alloy_primitives::B512;

// TODO: should we use `PublicKey` for this? Even when dealing with public keys we should try to
// prevent misuse
/// This represents an uncompressed secp256k1 public key.
/// This encodes the concatenation of the x and y components of the affine point in bytes.
pub type PeerId = B512;
