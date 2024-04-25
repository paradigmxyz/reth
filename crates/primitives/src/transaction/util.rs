#[cfg(feature = "secp256k1")]
pub(crate) mod secp256k1 {
    use super::*;
    use crate::{keccak256, Address, Signature};
    pub(crate) use ::secp256k1::Error;
    use ::secp256k1::{
        ecdsa::{RecoverableSignature, RecoveryId},
        Message, SecretKey, SECP256K1,
    };
    use revm_primitives::{B256, U256};

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    pub fn recover_signer_unchecked(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        let sig =
            RecoverableSignature::from_compact(&sig[0..64], RecoveryId::from_i32(sig[64] as i32)?)?;

        let public = SECP256K1.recover_ecdsa(&Message::from_slice(&msg[..32])?, &sig)?;
        Ok(public_key_to_address(&public.serialize_uncompressed()))
    }

    /// Signs message with the given secret key.
    /// Returns the corresponding signature.
    pub fn sign_message(secret: B256, message: B256) -> Result<Signature, secp256k1::Error> {
        let sec = SecretKey::from_slice(secret.as_ref())?;
        let s = SECP256K1.sign_ecdsa_recoverable(&Message::from_slice(&message[..])?, &sec);
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
    pub fn public_key_to_address(uncompressed_public_key: &[u8; 65]) -> Address {
        // strip out the first byte because that should be the SECP256K1_TAG_PUBKEY_UNCOMPRESSED
        // tag returned by libsecp's uncompressed pubkey serialization
        let hash = keccak256(&uncompressed_public_key[1..]);
        Address::from_slice(&hash[12..])
    }
}

#[cfg(not(feature = "secp256k1"))]
pub(crate) mod secp256k1 {
    use crate::{keccak256, Address, Signature};
    // pub(crate) use ::secp256k1::Error;
    // use ::secp256k1::{
    //     ecdsa::{RecoverableSignature, RecoveryId},
    //     Message, PublicKey, SecretKey, SECP256K1,
    // };
    use k256::ecdsa::{Error, RecoveryId, Signature as ECDSASignature, SigningKey, VerifyingKey};
    use revm_primitives::{B256, U256};

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    pub fn recover_signer_unchecked(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        let mut recid = sig[64];
        let sig = &sig[0..64];
        // parse signature
        let mut sig = ECDSASignature::from_slice(sig)?;
        // normalize signature and flip recovery id if needed.
        if let Some(sig_normalized) = sig.normalize_s() {
            sig = sig_normalized;
            recid ^= 1;
        }
        let recid = RecoveryId::from_byte(recid).expect("recovery ID is valid");
        // recover key
        let recovered_key = VerifyingKey::recover_from_prehash(&msg[..], &sig, recid)?;

        Ok(public_key_to_address(
            recovered_key.to_encoded_point(/* compress = */ false).as_bytes().try_into().unwrap(),
        ))
    }

    /// Signs message with the given secret key.
    /// Returns the corresponding signature.
    pub fn sign_message(secret: B256, message: B256) -> Result<Signature, Error> {
        let secret_key = SigningKey::from_slice(secret.as_ref())?;
        let (s, rec_id) = secret_key.sign_recoverable(message.as_slice())?;
        let sig_bytes = s.to_bytes();
        let data = sig_bytes.as_slice();

        let signature = Signature {
            r: U256::try_from_be_slice(&data[..32]).expect("The slice has at most 32 bytes"),
            s: U256::try_from_be_slice(&data[32..64]).expect("The slice has at most 32 bytes"),
            odd_y_parity: rec_id.is_y_odd(),
        };
        Ok(signature)
    }

    /// Converts a public key into an ethereum address by hashing the encoded public key with
    /// keccak256.
    pub fn public_key_to_address(uncompressed_public_key: &[u8; 65]) -> Address {
        // strip out the first byte because that should be the SECP256K1_TAG_PUBKEY_UNCOMPRESSED
        // tag returned by libsecp's uncompressed pubkey serialization
        let hash = keccak256(&uncompressed_public_key[1..]);
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

        let signer =
            secp256k1::recover_signer_unchecked(&sig, &hash).expect("Failed to recover signer");
        assert_eq!(signer, out);
    }
}
