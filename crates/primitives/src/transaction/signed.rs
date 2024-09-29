//! API of a signed transaction, w.r.t. network stack.

use alloy_primitives::Address;

/// A signed transaction.
pub trait SignedTransaction: Sized {
    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid following [EIP-2](https://eips.ethereum.org/EIPS/eip-2), see also [`recover_signer`].
    ///
    /// Note:
    ///
    /// This can fail for some early ethereum mainnet transactions pre EIP-2, use
    /// [`Self::recover_signer_unchecked`] if you want to recover the signer without ensuring that
    /// the signature has a low `s` value.
    fn recover_signer(&self) -> Option<Address>;

    /// Recover signer from signature and hash _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also
    /// [`recover_signer_unchecked`].
    fn recover_signer_unchecked(&self) -> Option<Address>;

    /// Output the length of the `encode_inner(out`, true). Note to assume that `with_header` is
    /// only `true`.
    fn payload_len_inner(&self) -> usize;

    /// Decodes an enveloped EIP-2718 typed transaction.
    ///
    /// This should _only_ be used internally in general transaction decoding methods,
    /// which have already ensured that the input is a typed transaction with the following format:
    /// `tx-type || rlp(tx-data)`
    ///
    /// Note that this format does not start with any RLP header, and instead starts with a single
    /// byte indicating the transaction type.
    ///
    /// CAUTION: this expects that `data` is `tx-type || rlp(tx-data)`
    fn decode_enveloped_typed_transaction(data: &mut &[u8]) -> alloy_rlp::Result<Self>;

    /// Returns the length without an RLP header - this is used for eth/68 sizes.
    fn length_without_header(&self) -> usize;
}
