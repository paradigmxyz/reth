//! Signature types and helpers

/// Re-exported signature type
pub use alloy_primitives::PrimitiveSignature as Signature;

#[cfg(test)]
mod tests {
    use crate::crypto::secp256k1::recover_signer;
    use alloy_primitives::{Address, PrimitiveSignature as Signature, B256, U256};
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
}
