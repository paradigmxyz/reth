use crate::{transaction::util::secp256k1, Address, B256, U256};
use alloy_primitives::Bytes;
use alloy_rlp::{Decodable, Encodable, Error as RlpError};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use reth_codecs::Compact;

/// The order of the secp256k1 curve, divided by two. Signatures that should be checked according
/// to EIP-2 should have an S value less than or equal to this.
///
/// `57896044618658097711785492504343953926418782139537452191302581570759080747168`
const SECP256K1N_HALF: U256 = U256::from_be_bytes([
    0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0x5D, 0x57, 0x6E, 0x73, 0x57, 0xA4, 0x50, 0x1D, 0xDF, 0xE9, 0x2F, 0x46, 0x68, 0x1B, 0x20, 0xA0,
]);

/// r, s: Values corresponding to the signature of the
/// transaction and used to determine the sender of
/// the transaction; formally Tr and Ts. This is expanded in Appendix F of yellow paper.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct Signature {
    /// The R field of the signature; the point on the curve.
    pub r: U256,
    /// The S field of the signature; the point on the curve.
    pub s: U256,
    /// yParity: Signature Y parity; formally Ty
    ///
    /// WARNING: if it's deprecated in favor of `alloy_primitives::Signature` be sure that parity
    /// storage deser matches.
    pub odd_y_parity: bool,
}

impl Signature {
    /// Returns the signature for the optimism deposit transactions, which don't include a
    /// signature.
    #[cfg(feature = "optimism")]
    pub const fn optimism_deposit_tx_signature() -> Self {
        Self { r: U256::ZERO, s: U256::ZERO, odd_y_parity: false }
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for Signature {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(&self.r.as_le_bytes());
        buf.put_slice(&self.s.as_le_bytes());
        self.odd_y_parity as usize
    }

    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        assert!(buf.len() >= 64);
        let r = U256::from_le_slice(&buf[0..32]);
        let s = U256::from_le_slice(&buf[32..64]);
        buf.advance(64);
        (Self { r, s, odd_y_parity: identifier != 0 }, buf)
    }
}

impl Signature {
    /// Output the length of the signature without the length of the RLP header, using the legacy
    /// scheme with EIP-155 support depends on `chain_id`.
    pub(crate) fn payload_len_with_eip155_chain_id(&self, chain_id: Option<u64>) -> usize {
        self.v(chain_id).length() + self.r.length() + self.s.length()
    }

    /// Encode the `v`, `r`, `s` values without a RLP header.
    /// Encodes the `v` value using the legacy scheme with EIP-155 support depends on `chain_id`.
    pub(crate) fn encode_with_eip155_chain_id(
        &self,
        out: &mut dyn alloy_rlp::BufMut,
        chain_id: Option<u64>,
    ) {
        self.v(chain_id).encode(out);
        self.r.encode(out);
        self.s.encode(out);
    }

    /// Output the `v` of the signature depends on `chain_id`
    #[inline]
    #[allow(clippy::missing_const_for_fn)]
    pub fn v(&self, chain_id: Option<u64>) -> u64 {
        if let Some(chain_id) = chain_id {
            // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
            self.odd_y_parity as u64 + chain_id * 2 + 35
        } else {
            #[cfg(feature = "optimism")]
            // pre bedrock system transactions were sent from the zero address as legacy
            // transactions with an empty signature
            //
            // NOTE: this is very hacky and only relevant for op-mainnet pre bedrock
            if *self == Self::optimism_deposit_tx_signature() {
                return 0
            }
            self.odd_y_parity as u64 + 27
        }
    }

    /// Decodes the `v`, `r`, `s` values without a RLP header.
    /// This will return a chain ID if the `v` value is [EIP-155](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-155.md) compatible.
    pub(crate) fn decode_with_eip155_chain_id(
        buf: &mut &[u8],
    ) -> alloy_rlp::Result<(Self, Option<u64>)> {
        let v = u64::decode(buf)?;
        let r: U256 = Decodable::decode(buf)?;
        let s: U256 = Decodable::decode(buf)?;

        if v < 35 {
            // non-EIP-155 legacy scheme, v = 27 for even y-parity, v = 28 for odd y-parity
            if v != 27 && v != 28 {
                #[cfg(feature = "optimism")]
                // pre bedrock system transactions were sent from the zero address as legacy
                // transactions with an empty signature
                //
                // NOTE: this is very hacky and only relevant for op-mainnet pre bedrock
                if v == 0 && r.is_zero() && s.is_zero() {
                    return Ok((Self { r, s, odd_y_parity: false }, None))
                }
            }
        }

        let (odd_y_parity, chain_id) = extract_chain_id(v)?;
        Ok((Self { r, s, odd_y_parity }, chain_id))
    }

    /// Output the length of the signature without the length of the RLP header
    pub fn payload_len(&self) -> usize {
        self.odd_y_parity.length() + self.r.length() + self.s.length()
    }

    /// Encode the `odd_y_parity`, `r`, `s` values without a RLP header.
    pub fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.odd_y_parity.encode(out);
        self.r.encode(out);
        self.s.encode(out);
    }

    /// Decodes the `odd_y_parity`, `r`, `s` values without a RLP header.
    pub fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self {
            odd_y_parity: Decodable::decode(buf)?,
            r: Decodable::decode(buf)?,
            s: Decodable::decode(buf)?,
        })
    }

    /// Recover signer from message hash, _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Using this for signature validation will succeed, even if the signature is malleable or not
    /// compliant with EIP-2. This is provided for compatibility with old signatures which have
    /// large `s` values.
    pub fn recover_signer_unchecked(&self, hash: B256) -> Option<Address> {
        let mut sig: [u8; 65] = [0; 65];

        sig[0..32].copy_from_slice(&self.r.to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&self.s.to_be_bytes::<32>());
        sig[64] = self.odd_y_parity as u8;

        // NOTE: we are removing error from underlying crypto library as it will restrain primitive
        // errors and we care only if recovery is passing or not.
        secp256k1::recover_signer_unchecked(&sig, &hash.0).ok()
    }

    /// Recover signer address from message hash. This ensures that the signature S value is
    /// greater than `secp256k1n / 2`, as specified in
    /// [EIP-2](https://eips.ethereum.org/EIPS/eip-2).
    ///
    /// If the S value is too large, then this will return `None`
    pub fn recover_signer(&self, hash: B256) -> Option<Address> {
        if self.s > SECP256K1N_HALF {
            return None
        }

        self.recover_signer_unchecked(hash)
    }

    /// Turn this signature into its byte
    /// (hex) representation.
    pub fn to_bytes(&self) -> [u8; 65] {
        let mut sig = [0u8; 65];
        sig[..32].copy_from_slice(&self.r.to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&self.s.to_be_bytes::<32>());
        let v = u8::from(self.odd_y_parity) + 27;
        sig[64] = v;
        sig
    }

    /// Turn this signature into its hex-encoded representation.
    pub fn to_hex_bytes(&self) -> Bytes {
        self.to_bytes().into()
    }

    /// Calculates a heuristic for the in-memory size of the [Signature].
    #[inline]
    pub const fn size(&self) -> usize {
        core::mem::size_of::<Self>()
    }
}

impl From<alloy_primitives::Signature> for Signature {
    fn from(value: alloy_primitives::Signature) -> Self {
        Self { r: value.r(), s: value.s(), odd_y_parity: value.v().y_parity() }
    }
}

/// Outputs (`odd_y_parity`, `chain_id`) from the `v` value.
/// This doesn't check validity of the `v` value for optimism.
#[inline]
pub const fn extract_chain_id(v: u64) -> alloy_rlp::Result<(bool, Option<u64>)> {
    if v < 35 {
        // non-EIP-155 legacy scheme, v = 27 for even y-parity, v = 28 for odd y-parity
        if v != 27 && v != 28 {
            return Err(RlpError::Custom("invalid Ethereum signature (V is not 27 or 28)"))
        }
        Ok((v == 28, None))
    } else {
        // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
        let odd_y_parity = ((v - 35) % 2) != 0;
        let chain_id = (v - 35) >> 1;
        Ok((odd_y_parity, Some(chain_id)))
    }
}

#[cfg(test)]
mod tests {
    use crate::{transaction::signature::SECP256K1N_HALF, Address, Signature, B256, U256};
    use alloy_primitives::{hex, hex::FromHex, Bytes};
    use std::str::FromStr;

    #[test]
    fn test_payload_len_with_eip155_chain_id() {
        // Select 1 as an arbitrary nonzero value for R and S, as v() always returns 0 for (0, 0).
        let signature = Signature { r: U256::from(1), s: U256::from(1), odd_y_parity: false };

        assert_eq!(3, signature.payload_len_with_eip155_chain_id(None));
        assert_eq!(3, signature.payload_len_with_eip155_chain_id(Some(1)));
        assert_eq!(4, signature.payload_len_with_eip155_chain_id(Some(47)));
    }

    #[test]
    fn test_v() {
        // Select 1 as an arbitrary nonzero value for R and S, as v() always returns 0 for (0, 0).
        let signature = Signature { r: U256::from(1), s: U256::from(1), odd_y_parity: false };
        assert_eq!(27, signature.v(None));
        assert_eq!(37, signature.v(Some(1)));

        let signature = Signature { r: U256::from(1), s: U256::from(1), odd_y_parity: true };
        assert_eq!(28, signature.v(None));
        assert_eq!(38, signature.v(Some(1)));
    }

    #[test]
    fn test_encode_and_decode_with_eip155_chain_id() {
        // Select 1 as an arbitrary nonzero value for R and S, as v() always returns 0 for (0, 0).
        let signature = Signature { r: U256::from(1), s: U256::from(1), odd_y_parity: false };

        let mut encoded = Vec::new();
        signature.encode_with_eip155_chain_id(&mut encoded, None);
        assert_eq!(encoded.len(), signature.payload_len_with_eip155_chain_id(None));
        let (decoded, chain_id) = Signature::decode_with_eip155_chain_id(&mut &*encoded).unwrap();
        assert_eq!(signature, decoded);
        assert_eq!(None, chain_id);

        let mut encoded = Vec::new();
        signature.encode_with_eip155_chain_id(&mut encoded, Some(1));
        assert_eq!(encoded.len(), signature.payload_len_with_eip155_chain_id(Some(1)));
        let (decoded, chain_id) = Signature::decode_with_eip155_chain_id(&mut &*encoded).unwrap();
        assert_eq!(signature, decoded);
        assert_eq!(Some(1), chain_id);
    }

    #[test]
    fn test_payload_len() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };
        assert_eq!(3, signature.payload_len());
    }

    #[test]
    fn test_encode_and_decode() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };

        let mut encoded = Vec::new();
        signature.encode(&mut encoded);
        assert_eq!(encoded.len(), signature.payload_len());
        let decoded = Signature::decode(&mut &*encoded).unwrap();
        assert_eq!(signature, decoded);
    }

    #[test]
    fn test_recover_signer() {
        let signature = Signature {
            r: U256::from_str(
                "18515461264373351373200002665853028612451056578545711640558177340181847433846",
            )
            .unwrap(),
            s: U256::from_str(
                "46948507304638947509940763649030358759909902576025900602547168820602576006531",
            )
            .unwrap(),
            odd_y_parity: false,
        };
        let hash =
            B256::from_str("daf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53")
                .unwrap();
        let signer = signature.recover_signer(hash).unwrap();
        let expected = Address::from_str("0x9d8a62f656a8d1615c1294fd71e9cfb3e4855a4f").unwrap();
        assert_eq!(expected, signer);
    }

    #[test]
    fn ensure_size_equals_sum_of_fields() {
        let signature = Signature {
            r: U256::from_str(
                "18515461264373351373200002665853028612451056578545711640558177340181847433846",
            )
            .unwrap(),
            s: U256::from_str(
                "46948507304638947509940763649030358759909902576025900602547168820602576006531",
            )
            .unwrap(),
            odd_y_parity: false,
        };

        assert!(signature.size() >= 65);
    }

    #[test]
    fn test_to_hex_bytes() {
        let signature = Signature {
            r: U256::from_str(
                "18515461264373351373200002665853028612451056578545711640558177340181847433846",
            )
            .unwrap(),
            s: U256::from_str(
                "46948507304638947509940763649030358759909902576025900602547168820602576006531",
            )
            .unwrap(),
            odd_y_parity: false,
        };

        let expected = Bytes::from_hex("0x28ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa63627667cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d831b").unwrap();
        assert_eq!(signature.to_hex_bytes(), expected);
    }

    #[test]
    fn eip_2_reject_high_s_value() {
        // This pre-homestead transaction has a high `s` value and should be rejected by the
        // `recover_signer` method:
        // https://etherscan.io/getRawTx?tx=0x9e6e19637bb625a8ff3d052b7c2fe57dc78c55a15d258d77c43d5a9c160b0384
        //
        // Block number: 46170
        let raw_tx = hex!("f86d8085746a52880082520894c93f2250589a6563f5359051c1ea25746549f0d889208686e75e903bc000801ba034b6fdc33ea520e8123cf5ac4a9ff476f639cab68980cd9366ccae7aef437ea0a0e517caa5f50e27ca0d1e9a92c503b4ccb039680c6d9d0c71203ed611ea4feb33");
        let tx = crate::transaction::TransactionSigned::decode_enveloped(&mut &raw_tx[..]).unwrap();
        let signature = tx.signature();

        // make sure we know it's greater than SECP256K1N_HALF
        assert!(signature.s > SECP256K1N_HALF);

        // recover signer, expect failure
        let hash = tx.hash();
        assert!(signature.recover_signer(hash).is_none());

        // use unchecked, ensure it succeeds (the signature is valid if not for EIP-2)
        assert!(signature.recover_signer_unchecked(hash).is_some());
    }
}
