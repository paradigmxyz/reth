use crate::{keccak256, Address};

pub(crate) mod secp256k1 {
    use super::*;
    use crate::Signature;
    pub(crate) use ::secp256k1::Error;
    use ::secp256k1::{
        ecdsa::{RecoverableSignature, RecoveryId},
        Message, PublicKey, SecretKey, SECP256K1,
    };
    use revm_primitives::{B256, U256};

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    pub fn recover_signer(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        let sig =
            RecoverableSignature::from_compact(&sig[0..64], RecoveryId::from_i32(sig[64] as i32)?)?;

        let public = SECP256K1.recover_ecdsa(&Message::from_slice(&msg[..32])?, &sig)?;
        Ok(public_key_to_address(public))
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
    pub fn public_key_to_address(public: PublicKey) -> Address {
        // strip out the first byte because that should be the SECP256K1_TAG_PUBKEY_UNCOMPRESSED
        // tag returned by libsecp's uncompressed pubkey serialization
        let hash = keccak256(&public.serialize_uncompressed()[1..]);
        Address::from_slice(&hash[12..])
    }
}
#[cfg(test)]
mod tests {

    use super::secp256k1;
    use crate::{hex_literal::hex, Address};

    #[test]
    fn sanity_ecrecover_call() {
        let sig = hex!("650acf9d3f5f0a2c799776a1254355d5f4061762a237396a99a0e0e3fc2bcd6729514a0dacb2e623ac4abd157cb18163ff942280db4d5caad66ddf941ba12e0300");
        let hash = hex!("47173285a8d7341e5e972fc677286384f802f8ef42a5ec5f03bbfa254cb01fad");
        let out: Address = hex!("c08b5542d177ac6686946920409741463a15dddb").into();

        assert_eq!(secp256k1::recover_signer(&sig, &hash), Ok(out));
    }
}
