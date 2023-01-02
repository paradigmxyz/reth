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

#[allow(dead_code)]
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
