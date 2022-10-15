use hmac::{Hmac, Mac};
use reth_primitives::{H256, H512 as PeerId};
use secp256k1::PublicKey;
use sha2::Sha256;
use sha3::{Digest, Keccak256};
use std::fmt::{self, Formatter};

pub fn keccak256(data: &[u8]) -> H256 {
    H256::from(Keccak256::digest(data).as_ref())
}

pub fn sha256(data: &[u8]) -> H256 {
    H256::from(Sha256::digest(data).as_ref())
}

pub fn hmac_sha256(key: &[u8], input: &[&[u8]], auth_data: &[u8]) -> H256 {
    let mut hmac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    for input in input {
        hmac.update(input);
    }
    hmac.update(auth_data);
    H256::from_slice(&*hmac.finalize().into_bytes())
}

pub fn pk2id(pk: &PublicKey) -> PeerId {
    PeerId::from_slice(&pk.serialize_uncompressed()[1..])
}

pub fn id2pk(id: PeerId) -> Result<PublicKey, secp256k1::Error> {
    let mut s = [0_u8; 65];
    s[0] = 4;
    s[1..].copy_from_slice(id.as_bytes());
    PublicKey::from_slice(&s)
}

pub fn hex_debug<T: AsRef<[u8]>>(s: &T, f: &mut Formatter<'_>) -> fmt::Result {
    f.write_str(&hex::encode(&s))
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
