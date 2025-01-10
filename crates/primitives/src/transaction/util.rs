pub(crate) mod secp256k1 {
    use super::*;
    use crate::{keccak256, Address, Signature};
    pub(crate) use ::secp256k1::Error;
    use ::secp256k1::{Message, SecretKey, SECP256K1};
    use revm_primitives::{B256, U256};

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    #[cfg(not(feature = "k256"))]
    pub fn recover_signer_unchecked(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        use ::secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
        let sig =
            RecoverableSignature::from_compact(&sig[0..64], RecoveryId::from_i32(sig[64] as i32)?)?;

        let public = SECP256K1.recover_ecdsa(&Message::from_digest(*msg), &sig)?;
        Ok(public_key_to_address(public))
    }

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    #[cfg(feature = "k256")]
    pub fn recover_signer_unchecked(
        sig: &[u8; 65],
        msg: &[u8; 32],
    ) -> Result<Address, k256::ecdsa::Error> {
        use k256::ecdsa::{RecoveryId, VerifyingKey};

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
    pub fn sign_message(secret: B256, message: B256) -> Result<Signature, secp256k1::Error> {
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
    #[cfg(not(feature = "k256"))]
    pub fn public_key_to_address(public: ::secp256k1::PublicKey) -> Address {
        // strip out the first byte because that should be the SECP256K1_TAG_PUBKEY_UNCOMPRESSED
        // tag returned by libsecp's uncompressed pubkey serialization
        let hash = keccak256(&public.serialize_uncompressed()[1..]);
        Address::from_slice(&hash[12..])
    }

    /// Converts a public key into an ethereum address by hashing the encoded public key with
    /// keccak256.
    #[cfg(feature = "k256")]
    pub fn public_key_to_address(public: k256::ecdsa::VerifyingKey) -> Address {
        let hash = keccak256(&public.to_encoded_point(/* compress = */ false).as_bytes()[1..]);
        Address::from_slice(&hash[12..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{address, hex};

    #[test]
    fn sanity_ecrecover_call() {
        let sig = hex!("650acf9d3f5f0a2c799776a1254355d5f4061762a237396a99a0e0e3fc2bcd6729514a0dacb2e623ac4abd157cb18163ff942280db4d5caad66ddf941ba12e0300");
        let hash = hex!("47173285a8d7341e5e972fc677286384f802f8ef42a5ec5f03bbfa254cb01fad");
        let out = address!("c08b5542d177ac6686946920409741463a15dddb");

        assert_eq!(secp256k1::recover_signer_unchecked(&sig, &hash), Ok(out));
    }
}
