use crate::{Address, Signature};
use revm_primitives::B256;

// Silence the `unused_imports` warning for the `k256` feature if both `secp256k1` and `k256` are
// enabled.
#[cfg(all(feature = "k256", feature = "secp256k1"))]
#[allow(unused_imports)]
use k256 as _;

#[cfg(feature = "secp256k1")]
mod impl_secp256k1 {
    use super::*;
    use crate::keccak256;
    pub(crate) use ::secp256k1::Error;
    use ::secp256k1::{
        ecdsa::{RecoverableSignature, RecoveryId},
        Message, PublicKey, SecretKey, SECP256K1,
    };
    use revm_primitives::U256;

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    pub fn recover_signer_unchecked(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        let sig =
            RecoverableSignature::from_compact(&sig[0..64], RecoveryId::from_i32(sig[64] as i32)?)?;

        let public = SECP256K1.recover_ecdsa(&Message::from_digest(*msg), &sig)?;
        Ok(public_key_to_address(public))
    }

    /// Signs message with the given secret key.
    /// Returns the corresponding signature.
    pub fn sign_message(secret: B256, message: B256) -> Result<Signature, Error> {
        let sec = SecretKey::from_slice(secret.as_ref())?;
        let s = SECP256K1.sign_ecdsa_recoverable(&Message::from_digest(message.0), &sec);
        let (rec_id, data) = s.serialize_compact();

        let signature = Signature {
            r: U256::try_from_be_slice(&data[..32]).expect("The slice has at most 32 bytes"),
            s: U256::try_from_be_slice(&data[32..64]).expect("The slice has at most 32 bytes"),
            odd_y_parity: rec_id.to_i32() != 0,
        };
        Ok(signature)
    }

    /// Converts a public key into an ethereum address by hashing the encoded public key with
    /// keccak256.
    pub fn public_key_to_address(public: PublicKey) -> Address {
        // strip out the first byte because that should be the SECP256K1_TAG_PUBKEY_UNCOMPRESSED
        // tag returned by libsecp's uncompressed pubkey serialization
        let hash = keccak256(&public.serialize_uncompressed()[1..]);
        Address::from_slice(&hash[12..])
    }
}

#[cfg(feature = "k256")]
// If both `secp256k1` and `k256` are enabled, then we need to silence the warnings
#[cfg_attr(all(feature = "k256", feature = "secp256k1"), allow(unused, unreachable_pub))]
mod impl_k256 {
    use super::*;
    use crate::keccak256;
    pub(crate) use ::k256::ecdsa::Error;
    use k256::ecdsa::{RecoveryId, SigningKey, VerifyingKey};
    use revm_primitives::U256;

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    pub fn recover_signer_unchecked(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        let mut signature = k256::ecdsa::Signature::from_slice(&sig[0..64])?;
        let mut recid = sig[64];

        // normalize signature and flip recovery id if needed.
        if let Some(sig_normalized) = signature.normalize_s() {
            signature = sig_normalized;
            recid ^= 1;
        }
        let recid = RecoveryId::from_byte(recid).expect("recovery ID is valid");

        // recover key
        let recovered_key = VerifyingKey::recover_from_prehash(&msg[..], &signature, recid)?;
        Ok(public_key_to_address(recovered_key))
    }

    /// Signs message with the given secret key.
    /// Returns the corresponding signature.
    pub fn sign_message(secret: B256, message: B256) -> Result<Signature, Error> {
        let sec = SigningKey::from_slice(secret.as_ref())?;
        let (s, rec_id) = sec.sign_recoverable(&message.0)?;
        let data = s.to_bytes();

        let signature = Signature {
            r: U256::try_from_be_slice(&data[..32]).expect("The slice has at most 32 bytes"),
            s: U256::try_from_be_slice(&data[32..64]).expect("The slice has at most 32 bytes"),
            odd_y_parity: rec_id.is_y_odd(),
        };
        Ok(signature)
    }

    /// Converts a public key into an ethereum address by hashing the encoded public key with
    /// keccak256.
    pub fn public_key_to_address(public: VerifyingKey) -> Address {
        let hash = keccak256(&public.to_encoded_point(/* compress = */ false).as_bytes()[1..]);
        Address::from_slice(&hash[12..])
    }
}

// Do not check `not(feature = "k256")` because then compilation with `--all-features` will fail.
#[cfg(feature = "secp256k1")]
pub(crate) mod secp256k1 {
    pub use super::impl_secp256k1::*;
}

#[cfg(all(feature = "k256", not(feature = "secp256k1")))]
pub(crate) mod secp256k1 {
    pub use super::impl_k256::*;
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "secp256k1")]
    #[test]
    fn sanity_ecrecover_call_secp256k1() {
        let sig = crate::hex!("650acf9d3f5f0a2c799776a1254355d5f4061762a237396a99a0e0e3fc2bcd6729514a0dacb2e623ac4abd157cb18163ff942280db4d5caad66ddf941ba12e0300");
        let hash = crate::hex!("47173285a8d7341e5e972fc677286384f802f8ef42a5ec5f03bbfa254cb01fad");
        let out = crate::address!("c08b5542d177ac6686946920409741463a15dddb");

        assert_eq!(super::impl_secp256k1::recover_signer_unchecked(&sig, &hash), Ok(out));
    }

    #[cfg(feature = "k256")]
    #[test]
    fn sanity_ecrecover_call_k256() {
        let sig = crate::hex!("650acf9d3f5f0a2c799776a1254355d5f4061762a237396a99a0e0e3fc2bcd6729514a0dacb2e623ac4abd157cb18163ff942280db4d5caad66ddf941ba12e0300");
        let hash = crate::hex!("47173285a8d7341e5e972fc677286384f802f8ef42a5ec5f03bbfa254cb01fad");
        let out = crate::address!("c08b5542d177ac6686946920409741463a15dddb");

        assert_eq!(super::impl_k256::recover_signer_unchecked(&sig, &hash).ok(), Some(out));
    }
}
