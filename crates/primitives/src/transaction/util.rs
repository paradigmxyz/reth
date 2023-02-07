use crate::{keccak256, Address};

pub(crate) mod secp256k1 {
    use super::*;
    use ::secp256k1::{
        ecdsa::{RecoverableSignature, RecoveryId},
        Error, Message, SECP256K1,
    };

    /// secp256k1 signer recovery
    pub(crate) fn recover(sig: &[u8; 65], msg: &[u8; 32]) -> Result<Address, Error> {
        let sig =
            RecoverableSignature::from_compact(&sig[0..64], RecoveryId::from_i32(sig[64] as i32)?)?;

        let public = SECP256K1.recover_ecdsa(&Message::from_slice(&msg[..32])?, &sig)?;
        let hash = keccak256(&public.serialize_uncompressed()[1..]);
        Ok(Address::from_slice(&hash[12..]))
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

        assert_eq!(secp256k1::recover(&sig, &hash), Ok(out));
    }
}
