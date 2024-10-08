//! Signature abstraction

use alloy_primitives::{Address, Parity, B256, U256};

/// Reth extension for alloy type [`Signature`](alloy_primitives::Signature).
pub trait Signature: Sized + Send + Sync {
    /// Decodes RLP-encoded signature, w.r.t. chain ID.
    fn decode_with_eip155_chain_id(buf: &mut &[u8]) -> alloy_rlp::Result<(Self, Option<u64>)>;

    /// Recover signer from message hash, _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Using this for signature validation will succeed, even if the signature is malleable or not
    /// compliant with EIP-2. This is provided for compatibility with old signatures which have
    /// large `s` values.
    fn recover_signer_unchecked(&self, hash: B256) -> Option<Address>;

    /// Recover signer address from message hash. This ensures that the signature S value is
    /// greater than `secp256k1n / 2`, as specified in
    /// [EIP-2](https://eips.ethereum.org/EIPS/eip-2).
    ///
    /// If the S value is too large, then this will return `None`
    fn recover_signer(&self, hash: B256) -> Option<Address>;

    /// Returns [`Parity`] value based on `chain_id` for legacy transaction signature.
    fn legacy_parity(&self, chain_id: Option<u64>) -> Parity;

    /// Returns a signature with the given chain ID applied to the `v` value.
    fn with_eip155_parity(&self, chain_id: Option<u64>) -> Self;

    /// Outputs (`odd_y_parity`, `chain_id`) from the `v` value.
    /// This doesn't check validity of the `v` value for optimism.
    fn extract_chain_id(v: u64) -> alloy_rlp::Result<(bool, Option<u64>)>;
}

// todo: add optimism type that wraps Signature, to impl separately for OP to account for system
// null signature
impl Signature for alloy_primitives::Signature {
    fn decode_with_eip155_chain_id(buf: &mut &[u8]) -> alloy_rlp::Result<(Self, Option<u64>)> {
        let v: Parity = alloy_rlp::Decodable::decode(buf)?;
        let r: U256 = alloy_rlp::Decodable::decode(buf)?;
        let s: U256 = alloy_rlp::Decodable::decode(buf)?;

        if matches!(v, Parity::Parity(_)) {
            return Err(alloy_rlp::Error::Custom("invalid parity for legacy transaction"));
        }

        Ok((Self::new(r, s, v), v.chain_id()))
    }

    fn recover_signer_unchecked(&self, hash: B256) -> Option<Address> {
        let mut sig: [u8; 65] = [0; 65];

        sig[0..32].copy_from_slice(&self.r().to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&self.s().to_be_bytes::<32>());
        sig[64] = self.v().y_parity_byte();

        // NOTE: we are removing error from underlying crypto library as it will restrain primitive
        // errors and we care only if recovery is passing or not.
        secp256k1::recover_signer_unchecked(&sig, &hash.0).ok()
    }

    fn recover_signer(&self, hash: B256) -> Option<Address> {
        if self.s() > secp256k1::SECP256K1N_HALF {
            return None
        }

        self.recover_signer_unchecked(hash)
    }

    fn legacy_parity(&self, chain_id: Option<u64>) -> Parity {
        if let Some(chain_id) = chain_id {
            Parity::Parity(self.v().y_parity()).with_chain_id(chain_id)
        } else {
            Parity::NonEip155(self.v().y_parity())
        }
    }

    fn with_eip155_parity(&self, chain_id: Option<u64>) -> Self {
        Self::new(self.r(), self.s(), self.legacy_parity(chain_id))
    }

    #[inline]
    fn extract_chain_id(v: u64) -> alloy_rlp::Result<(bool, Option<u64>)> {
        if v < 35 {
            // non-EIP-155 legacy scheme, v = 27 for even y-parity, v = 28 for odd y-parity
            if v != 27 && v != 28 {
                return Err(alloy_rlp::Error::Custom(
                    "invalid Ethereum signature (V is not 27 or 28)",
                ))
            }
            Ok((v == 28, None))
        } else {
            // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
            let odd_y_parity = ((v - 35) % 2) != 0;
            let chain_id = (v - 35) >> 1;
            Ok((odd_y_parity, Some(chain_id)))
        }
    }
}

pub mod secp256k1 {
    //! Utilities for SECP256k1 signatures.

    use alloy_primitives::{keccak256, Address, U256};
    use secp256k1::{
        ecdsa::{RecoverableSignature, RecoveryId},
        Message, PublicKey, SECP256K1,
    };

    /// The order of the secp256k1 curve, divided by two. Signatures that should be checked
    /// according to EIP-2 should have an S value less than or equal to this.
    ///
    /// `57896044618658097711785492504343953926418782139537452191302581570759080747168`
    pub const SECP256K1N_HALF: U256 = U256::from_be_bytes([
        0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0x5D, 0x57, 0x6E, 0x73, 0x57, 0xA4, 0x50, 0x1D, 0xDF, 0xE9, 0x2F, 0x46, 0x68, 0x1B,
        0x20, 0xA0,
    ]);

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    pub fn recover_signer_unchecked(
        sig: &[u8; 65],
        msg: &[u8; 32],
    ) -> Result<Address, secp256k1::Error> {
        let sig =
            RecoverableSignature::from_compact(&sig[0..64], RecoveryId::from_i32(sig[64] as i32)?)?;

        let public = SECP256K1.recover_ecdsa(&Message::from_digest(*msg), &sig)?;
        Ok(public_key_to_address(public))
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
