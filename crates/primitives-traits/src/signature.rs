//! Signature abstraction tba

/// Decodes [`Signature`] w.r.t. chain ID.
pub fn decode_with_eip155_chain_id(buf: &mut &[u8]) -> alloy_rlp::Result<(Signature, Option<u64>)> {
    let v: Parity = Decodable::decode(buf)?;
    let r: U256 = Decodable::decode(buf)?;
    let s: U256 = Decodable::decode(buf)?;

    #[cfg(not(feature = "optimism"))]
    if matches!(v, Parity::Parity(_)) {
        return Err(alloy_rlp::Error::Custom("invalid parity for legacy transaction"));
    }

    #[cfg(feature = "optimism")]
    // pre bedrock system transactions were sent from the zero address as legacy
    // transactions with an empty signature
    //
    // NOTE: this is very hacky and only relevant for op-mainnet pre bedrock
    if matches!(v, Parity::Parity(false)) && r.is_zero() && s.is_zero() {
        return Ok((Signature::new(r, s, Parity::Parity(false)), None))
    }

    Ok((Signature::new(r, s, v), v.chain_id()))
}
