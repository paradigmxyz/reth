//! Signature abstraction

use alloy_primitives::{Address, Parity, B256, U256};

/// Reth extension for alloy type [`Signature`](alloy_primitives::Signature).
pub trait Signature: Sized + Send + Sync {
    /// Returns ref to `r` value.
    fn r(&self) -> U256;

    /// Returns ref to `s` value.
    fn s(&self) -> U256;

    /// Returns ref to `v` value.
    fn v(&self) -> Parity;

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
