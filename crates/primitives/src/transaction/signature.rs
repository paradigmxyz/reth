use crate::{transaction::util::secp256k1, Address, H256, U256};
use bytes::Buf;
use reth_codecs::{derive_arbitrary, Compact};
use reth_rlp::{Decodable, DecodeError, Encodable};
use serde::{Deserialize, Serialize};

/// r, s: Values corresponding to the signature of the
/// transaction and used to determine the sender of
/// the transaction; formally Tr and Ts. This is expanded in Appendix F of yellow paper.
#[derive_arbitrary(compact)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Signature {
    /// The R field of the signature; the point on the curve.
    pub r: U256,
    /// The S field of the signature; the point on the curve.
    pub s: U256,
    /// yParity: Signature Y parity; formally Ty
    pub odd_y_parity: bool,
}

impl Compact for Signature {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(self.r.as_le_bytes().as_ref());
        buf.put_slice(self.s.as_le_bytes().as_ref());
        self.odd_y_parity as usize
    }

    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        let r = U256::try_from_le_slice(&buf[..32]).expect("qed");
        buf.advance(32);

        let s = U256::try_from_le_slice(&buf[..32]).expect("qed");
        buf.advance(32);

        (Signature { r, s, odd_y_parity: identifier != 0 }, buf)
    }
}

impl Signature {
    /// Output the length of the signature without the length of the RLP header, using the legacy
    /// scheme with EIP-155 support depends on chain_id.
    pub(crate) fn payload_len_with_eip155_chain_id(&self, chain_id: Option<u64>) -> usize {
        self.v(chain_id).length() + self.r.length() + self.s.length()
    }

    /// Encode the `v`, `r`, `s` values without a RLP header.
    /// Encodes the `v` value using the legacy scheme with EIP-155 support depends on chain_id.
    pub(crate) fn encode_with_eip155_chain_id(
        &self,
        out: &mut dyn reth_rlp::BufMut,
        chain_id: Option<u64>,
    ) {
        self.v(chain_id).encode(out);
        self.r.encode(out);
        self.s.encode(out);
    }

    /// Output the `v` of the signature depends on chain_id
    #[inline]
    pub fn v(&self, chain_id: Option<u64>) -> u64 {
        if let Some(chain_id) = chain_id {
            // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
            self.odd_y_parity as u64 + chain_id * 2 + 35
        } else {
            self.odd_y_parity as u64 + 27
        }
    }

    /// Decodes the `v`, `r`, `s` values without a RLP header.
    /// This will return a chain ID if the `v` value is [EIP-155](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-155.md) compatible.
    pub(crate) fn decode_with_eip155_chain_id(
        buf: &mut &[u8],
    ) -> Result<(Self, Option<u64>), DecodeError> {
        let v = u64::decode(buf)?;
        let r = Decodable::decode(buf)?;
        let s = Decodable::decode(buf)?;
        if v >= 35 {
            // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
            let odd_y_parity = ((v - 35) % 2) != 0;
            let chain_id = (v - 35) >> 1;
            Ok((Signature { r, s, odd_y_parity }, Some(chain_id)))
        } else {
            // non-EIP-155 legacy scheme, v = 27 for even y-parity, v = 28 for odd y-parity
            if v != 27 && v != 28 {
                return Err(DecodeError::Custom("invalid Ethereum signature (V is not 27 or 28)"))
            }
            let odd_y_parity = v == 28;
            Ok((Signature { r, s, odd_y_parity }, None))
        }
    }

    /// Output the length of the signature without the length of the RLP header
    pub(crate) fn payload_len(&self) -> usize {
        self.odd_y_parity.length() + self.r.length() + self.s.length()
    }

    /// Encode the `odd_y_parity`, `r`, `s` values without a RLP header.
    pub(crate) fn encode(&self, out: &mut dyn reth_rlp::BufMut) {
        self.odd_y_parity.encode(out);
        self.r.encode(out);
        self.s.encode(out);
    }

    /// Decodes the `odd_y_parity`, `r`, `s` values without a RLP header.
    pub(crate) fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        Ok(Signature {
            odd_y_parity: Decodable::decode(buf)?,
            r: Decodable::decode(buf)?,
            s: Decodable::decode(buf)?,
        })
    }

    /// Recover signer address from message hash.
    pub fn recover_signer(&self, hash: H256) -> Option<Address> {
        let mut sig: [u8; 65] = [0; 65];

        sig[0..32].copy_from_slice(&self.r.to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&self.s.to_be_bytes::<32>());
        sig[64] = self.odd_y_parity as u8;

        // NOTE: we are removing error from underlying crypto library as it will restrain primitive
        // errors and we care only if recovery is passing or not.
        secp256k1::recover_signer(&sig, hash.as_fixed_bytes()).ok()
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
}

#[cfg(test)]
mod tests {
    use crate::{Address, Signature, H256, U256};
    use bytes::BytesMut;
    use std::str::FromStr;

    #[test]
    fn test_payload_len_with_eip155_chain_id() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };

        assert_eq!(3, signature.payload_len_with_eip155_chain_id(None));
        assert_eq!(3, signature.payload_len_with_eip155_chain_id(Some(1)));
        assert_eq!(4, signature.payload_len_with_eip155_chain_id(Some(47)));
    }

    #[test]
    fn test_v() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };
        assert_eq!(27, signature.v(None));
        assert_eq!(37, signature.v(Some(1)));

        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: true };
        assert_eq!(28, signature.v(None));
        assert_eq!(38, signature.v(Some(1)));
    }

    #[test]
    fn test_encode_and_decode_with_eip155_chain_id() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };

        let mut encoded = BytesMut::new();
        signature.encode_with_eip155_chain_id(&mut encoded, None);
        assert_eq!(encoded.len(), signature.payload_len_with_eip155_chain_id(None));
        let (decoded, chain_id) = Signature::decode_with_eip155_chain_id(&mut &*encoded).unwrap();
        assert_eq!(signature, decoded);
        assert_eq!(None, chain_id);

        let mut encoded = BytesMut::new();
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

        let mut encoded = BytesMut::new();
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
            H256::from_str("daf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53")
                .unwrap();
        let signer = signature.recover_signer(hash).unwrap();
        let expected = Address::from_str("0x9d8a62f656a8d1615c1294fd71e9cfb3e4855a4f").unwrap();
        assert_eq!(expected, signer);
    }
}
