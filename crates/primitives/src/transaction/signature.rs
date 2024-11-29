use crate::transaction::util::secp256k1;
use alloy_consensus::transaction::from_eip155_value;
use alloy_primitives::{Address, PrimitiveSignature as Signature, B256, U256};
use alloy_rlp::Decodable;

/// The order of the secp256k1 curve, divided by two. Signatures that should be checked according
/// to EIP-2 should have an S value less than or equal to this.
///
/// `57896044618658097711785492504343953926418782139537452191302581570759080747168`
const SECP256K1N_HALF: U256 = U256::from_be_bytes([
    0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0x5D, 0x57, 0x6E, 0x73, 0x57, 0xA4, 0x50, 0x1D, 0xDF, 0xE9, 0x2F, 0x46, 0x68, 0x1B, 0x20, 0xA0,
]);

pub(crate) fn decode_with_eip155_chain_id(
    buf: &mut &[u8],
) -> alloy_rlp::Result<(Signature, Option<u64>)> {
    let v = Decodable::decode(buf)?;
    let r: U256 = Decodable::decode(buf)?;
    let s: U256 = Decodable::decode(buf)?;

    let Some((parity, chain_id)) = from_eip155_value(v) else {
        // pre bedrock system transactions were sent from the zero address as legacy
        // transactions with an empty signature
        //
        // NOTE: this is very hacky and only relevant for op-mainnet pre bedrock
        #[cfg(feature = "optimism")]
        if v == 0 && r.is_zero() && s.is_zero() {
            return Ok((Signature::new(r, s, false), None))
        }
        return Err(alloy_rlp::Error::Custom("invalid parity for legacy transaction"))
    };

    Ok((Signature::new(r, s, parity), chain_id))
}

/// Recover signer from message hash, _without ensuring that the signature has a low `s`
/// value_.
///
/// Using this for signature validation will succeed, even if the signature is malleable or not
/// compliant with EIP-2. This is provided for compatibility with old signatures which have
/// large `s` values.
pub fn recover_signer_unchecked(signature: &Signature, hash: B256) -> Option<Address> {
    let mut sig: [u8; 65] = [0; 65];

    sig[0..32].copy_from_slice(&signature.r().to_be_bytes::<32>());
    sig[32..64].copy_from_slice(&signature.s().to_be_bytes::<32>());
    sig[64] = signature.v() as u8;

    // NOTE: we are removing error from underlying crypto library as it will restrain primitive
    // errors and we care only if recovery is passing or not.
    secp256k1::recover_signer_unchecked(&sig, &hash.0).ok()
}

/// Recover signer address from message hash. This ensures that the signature S value is
/// greater than `secp256k1n / 2`, as specified in
/// [EIP-2](https://eips.ethereum.org/EIPS/eip-2).
///
/// If the S value is too large, then this will return `None`
pub fn recover_signer(signature: &Signature, hash: B256) -> Option<Address> {
    if signature.s() > SECP256K1N_HALF {
        return None
    }

    recover_signer_unchecked(signature, hash)
}

#[cfg(test)]
mod tests {
    use crate::transaction::signature::{
        recover_signer, recover_signer_unchecked, SECP256K1N_HALF,
    };
    use alloy_eips::eip2718::Decodable2718;
    use alloy_primitives::{hex, Address, PrimitiveSignature as Signature, B256, U256};
    use reth_primitives_traits::SignedTransaction;
    use std::str::FromStr;

    #[test]
    fn test_recover_signer() {
        let signature = Signature::new(
            U256::from_str(
                "18515461264373351373200002665853028612451056578545711640558177340181847433846",
            )
            .unwrap(),
            U256::from_str(
                "46948507304638947509940763649030358759909902576025900602547168820602576006531",
            )
            .unwrap(),
            false,
        );
        let hash =
            B256::from_str("daf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53")
                .unwrap();
        let signer = recover_signer(&signature, hash).unwrap();
        let expected = Address::from_str("0x9d8a62f656a8d1615c1294fd71e9cfb3e4855a4f").unwrap();
        assert_eq!(expected, signer);
    }

    #[test]
    fn eip_2_reject_high_s_value() {
        // This pre-homestead transaction has a high `s` value and should be rejected by the
        // `recover_signer` method:
        // https://etherscan.io/getRawTx?tx=0x9e6e19637bb625a8ff3d052b7c2fe57dc78c55a15d258d77c43d5a9c160b0384
        //
        // Block number: 46170
        let raw_tx = hex!("f86d8085746a52880082520894c93f2250589a6563f5359051c1ea25746549f0d889208686e75e903bc000801ba034b6fdc33ea520e8123cf5ac4a9ff476f639cab68980cd9366ccae7aef437ea0a0e517caa5f50e27ca0d1e9a92c503b4ccb039680c6d9d0c71203ed611ea4feb33");
        let tx = crate::transaction::TransactionSigned::decode_2718(&mut &raw_tx[..]).unwrap();
        let signature = tx.signature();

        // make sure we know it's greater than SECP256K1N_HALF
        assert!(signature.s() > SECP256K1N_HALF);

        // recover signer, expect failure
        let hash = tx.hash();
        assert!(recover_signer(signature, hash).is_none());

        // use unchecked, ensure it succeeds (the signature is valid if not for EIP-2)
        assert!(recover_signer_unchecked(signature, hash).is_some());
    }
}
