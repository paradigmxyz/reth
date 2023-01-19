use crate::{transaction::util::secp256k1, Address, H256, U256};
use reth_codecs::{main_codec, Compact};
use reth_rlp::{Decodable, DecodeError, Encodable};

/// r, s: Values corresponding to the signature of the
/// transaction and used to determine the sender of
/// the transaction; formally Tr and Ts. This is expanded in Appendix F of yellow paper.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Signature {
    /// The R field of the signature; the point on the curve.
    pub r: U256,
    /// The S field of the signature; the point on the curve.
    pub s: U256,
    /// yParity: Signature Y parity; formally Ty
    pub odd_y_parity: bool,
}

impl Signature {
    /// Encode the `v`, `r`, `s` values without a RLP header.
    /// Encodes the `v` value using the legacy scheme without EIP-155.
    pub(crate) fn encode_inner_legacy(&self, out: &mut dyn reth_rlp::BufMut) {
        (self.odd_y_parity as u8 + 27).encode(out);
        self.r.encode(out);
        self.s.encode(out);
    }

    /// Output the length of the signature without the length of the RLP header, using the legacy
    /// scheme without EIP-155.
    pub(crate) fn payload_len_legacy(&self) -> usize {
        (self.odd_y_parity as u8 + 27).length() + self.r.length() + self.s.length()
    }

    /// Encode the `v`, `r`, `s` values without a RLP header.
    /// Encodes the `v` value with EIP-155 support, using the specified chain ID.
    pub(crate) fn encode_eip155_inner(&self, out: &mut dyn reth_rlp::BufMut, chain_id: u64) {
        // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
        let v = chain_id * 2 + 35 + self.odd_y_parity as u64;
        v.encode(out);
        self.r.encode(out);
        self.s.encode(out);
    }

    /// Output the length of the signature without the length of the RLP header, with EIP-155
    /// support.
    pub(crate) fn eip155_payload_len(&self, chain_id: u64) -> usize {
        // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
        let v = chain_id * 2 + 35 + self.odd_y_parity as u64;
        v.length() + self.r.length() + self.s.length()
    }

    /// Decodes the `v`, `r`, `s` values without a RLP header.
    /// This will return a chain ID if the `v` value is EIP-155 compatible.
    pub(crate) fn decode_eip155_inner(buf: &mut &[u8]) -> Result<(Self, Option<u64>), DecodeError> {
        let v = u64::decode(buf)?;
        let r = Decodable::decode(buf)?;
        let s = Decodable::decode(buf)?;
        if v >= 35 {
            // EIP-155: v = {0, 1} + CHAIN_ID * 2 + 35
            let odd_y_parity = ((v - 35) % 2) != 0;
            let chain_id = (v - 35) >> 1;
            Ok((Signature { r, s, odd_y_parity }, Some(chain_id)))
        } else {
            // non-EIP-155 legacy scheme
            let odd_y_parity = (v - 27) != 0;
            Ok((Signature { r, s, odd_y_parity }, None))
        }
    }

    /// Recover signature from hash.
    pub(crate) fn recover_signer(&self, hash: H256) -> Option<Address> {
        let mut sig: [u8; 65] = [0; 65];

        sig[0..32].copy_from_slice(&self.r.to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&self.s.to_be_bytes::<32>());
        sig[64] = self.odd_y_parity as u8;

        // NOTE: we are removing error from underlying crypto library as it will restrain primitive
        // errors and we care only if recovery is passing or not.
        secp256k1::recover(&sig, hash.as_fixed_bytes()).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bytes::BytesMut;

    use crate::{Address, Signature, H256, U256};

    #[test]
    fn test_encode_inner_legacy() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };
        let mut encoded = BytesMut::new();
        signature.encode_inner_legacy(&mut encoded);
        let expected = 27u8;
        assert_eq!(expected, encoded.as_ref()[0]);

        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: true };
        let mut encoded = BytesMut::new();
        signature.encode_inner_legacy(&mut encoded);
        let expected = 28u8;
        assert_eq!(expected, encoded.as_ref()[0]);
    }

    #[test]
    fn test_payload_len_legacy() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };
        let len = signature.payload_len_legacy();
        assert_eq!(3, len);
    }

    #[test]
    fn test_encode_eip155_inner() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };
        let mut encoded = BytesMut::new();
        signature.encode_eip155_inner(&mut encoded, 1);
        let expected = 37u8;
        assert_eq!(expected, encoded.as_ref()[0]);

        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: true };
        let mut encoded = BytesMut::new();
        signature.encode_eip155_inner(&mut encoded, 1);
        let expected = 38u8;
        assert_eq!(expected, encoded.as_ref()[0]);
    }

    #[test]
    fn test_eip155_payload_len() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };
        let len = signature.eip155_payload_len(1);
        assert_eq!(3, len);

        let len = signature.eip155_payload_len(47);
        assert_eq!(4, len);
    }

    #[test]
    fn test_decode_eip155_inner() {
        let signature = Signature { r: U256::default(), s: U256::default(), odd_y_parity: false };

        let mut encoded = BytesMut::new();
        signature.encode_inner_legacy(&mut encoded);
        let (decoded, chain_id) = Signature::decode_eip155_inner(&mut &*encoded).unwrap();
        assert_eq!(signature, decoded);
        assert_eq!(None, chain_id);

        let mut encoded = BytesMut::new();
        signature.encode_eip155_inner(&mut encoded, 1);
        let (decoded, chain_id) = Signature::decode_eip155_inner(&mut &*encoded).unwrap();
        assert_eq!(signature, decoded);
        assert_eq!(Some(1), chain_id);
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
