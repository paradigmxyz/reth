use crate::{keccak256, Address};

pub mod secp256k1 {
    use super::*;
    use core::convert::TryFrom;
    use k256::{
        ecdsa::{recoverable, Error},
        elliptic_curve::sec1::ToEncodedPoint,
        PublicKey as K256PublicKey,
    };

    pub fn ecrecover(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        let sig = recoverable::Signature::try_from(sig.as_ref())?;
        let verify_key = sig.recover_verifying_key_from_digest_bytes(msg.into())?;
        let public_key = K256PublicKey::from(&verify_key);
        let public_key = public_key.to_encoded_point(/* compress = */ false);
        let public_key = public_key.as_bytes();
        let hash = keccak256(&public_key[1..]);
        Ok(Address::from_slice(&hash[12..]))
    }
}
