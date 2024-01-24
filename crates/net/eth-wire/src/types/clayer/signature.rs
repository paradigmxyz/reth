//! Implementation of consensus layer messages[ClayerSignature]
use alloy_rlp::{Decodable, Encodable};
use reth_codecs::derive_arbitrary;
use reth_primitives::{sign_message, Address, Signature, B256};
use secp256k1::SecretKey;
use serde::{Deserialize, Serialize};

// use super::message::ClayerConsensusMessageError;

/// Consensus layer signature
#[derive_arbitrary(rlp)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct ClayerSignature(pub Signature);

impl Encodable for ClayerSignature {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.0.encode(out)
    }

    fn length(&self) -> usize {
        self.0.payload_len()
    }
}

impl Decodable for ClayerSignature {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Signature::decode(buf)?))
    }
}

// impl ClayerSignature {
//     /// Create new signature
//     pub fn sign_hash(
//         sk: SecretKey,
//         signature_hash: B256,
//     ) -> std::result::Result<ClayerSignature, ClayerConsensusMessageError> {
//         let signature = sign_message(B256::from_slice(&sk.secret_bytes()[..]), signature_hash);
//         let signature = signature.map_err(|_| ClayerConsensusMessageError::CouldNotSign)?;
//         Ok(Self { 0: signature })
//     }

//     /// Recover signer from signature and hash.
//     pub fn recover_signer(&self, hash: B256) -> Option<Address> {
//         self.0.recover_signer(hash)
//     }
// }

#[cfg(test)]
mod tests {
    use reth_primitives::{hex, Bytes};
    use secp256k1::{PublicKey, SecretKey, SECP256K1};
    use std::str::FromStr;

    #[test]
    fn public_test() {
        let sk = SecretKey::from_slice(&hex!(
            "869d6ecf5211f1cc60418a13b9d870b22959d0c16f02bec714c960dd2298a32d"
        ))
        .unwrap();

        println!("sk: {:?}", sk.secret_bytes());

        let pk = PublicKey::from_secret_key(SECP256K1, &sk);
        println!("pk: {:?}", pk.serialize_uncompressed());
        println!("pk: {:?}", pk.to_string());
        println!("pk: {:?}", hex::encode(pk.serialize()));

        let pk2 = PublicKey::from_str(&hex::encode(pk.serialize())).unwrap();
        println!("pk2: {:?}", pk2.to_string());

        let b = Bytes::copy_from_slice(pk.serialize().as_ref());

        // println!("b: {:?}", hex::encode(b));
        let pk3 = PublicKey::from_slice(&b).unwrap();
        println!("pk3: {:?}", pk3.to_string());
    }
}
