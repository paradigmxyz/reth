//! Utility functions for hashing and encoding.

use hmac::{Hmac, Mac};
use reth_primitives::{B256, B512 as PeerId};
use secp256k1::PublicKey;
use sha2::{Digest, Sha256};

/// Hashes the input data with SHA256.
pub(crate) fn sha256(data: &[u8]) -> B256 {
    B256::from(Sha256::digest(data).as_ref())
}

/// Produces a HMAC_SHA256 digest of the `input_data` and `auth_data` with the given `key`.
/// This is done by accumulating each slice in `input_data` into the HMAC state, then accumulating
/// the `auth_data` and returning the resulting digest.
pub(crate) fn hmac_sha256(key: &[u8], input: &[&[u8]], auth_data: &[u8]) -> B256 {
    let mut hmac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    for input in input {
        hmac.update(input);
    }
    hmac.update(auth_data);
    B256::from_slice(&hmac.finalize().into_bytes())
}

/// Converts a [secp256k1::PublicKey] to a [PeerId] by stripping the
/// SECP256K1_TAG_PUBKEY_UNCOMPRESSED tag and storing the rest of the slice in the [PeerId].
pub fn pk2id(pk: &PublicKey) -> PeerId {
    PeerId::from_slice(&pk.serialize_uncompressed()[1..])
}

/// Converts a [PeerId] to a [secp256k1::PublicKey] by prepending the [PeerId] bytes with the
/// SECP256K1_TAG_PUBKEY_UNCOMPRESSED tag.
pub(crate) fn id2pk(id: PeerId) -> Result<PublicKey, secp256k1::Error> {
    // NOTE: B512 is used as a PeerId not because it represents a hash, but because 512 bits is
    // enough to represent an uncompressed public key.
    let mut s = [0u8; 65];
    // SECP256K1_TAG_PUBKEY_UNCOMPRESSED = 0x04
    // see: https://github.com/bitcoin-core/secp256k1/blob/master/include/secp256k1.h#L211
    s[0] = 4;
    s[1..].copy_from_slice(id.as_slice());
    PublicKey::from_slice(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secp256k1::{SecretKey, SECP256K1};

    #[test]
    fn pk2id2pk() {
        let prikey = SecretKey::new(&mut secp256k1::rand::thread_rng());
        let pubkey = PublicKey::from_secret_key(SECP256K1, &prikey);
        assert_eq!(pubkey, id2pk(pk2id(&pubkey)).unwrap());
    }
}
