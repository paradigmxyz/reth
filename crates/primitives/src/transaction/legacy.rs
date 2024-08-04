pub use alloy_consensus::TxLegacy;

#[cfg(test)]
mod tests {
    use super::TxLegacy;
    use crate::{
        transaction::{signature::Signature, TxKind},
        Address, Transaction, TransactionSigned, B256, U256,
    };

    #[test]
    fn recover_signer_legacy() {
        use crate::hex_literal::hex;

        let signer: Address = hex!("398137383b3d25c92898c656696e41950e47316b").into();
        let hash: B256 =
            hex!("bb3a336e3f823ec18197f1e13ee875700f08f03e2cab75f0d0b118dabb44cba0").into();

        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce: 0x18,
            gas_price: 0xfa56ea00,
            gas_limit: 119902,
            to: TxKind::Call(hex!("06012c8cf97bead5deae237070f9587f8e7a266d").into()),
            value: U256::from(0x1c6bf526340000u64),
            input:  hex!("f7d8c88300000000000000000000000000000000000000000000000000000000000cee6100000000000000000000000000000000000000000000000000000000000ac3e1").into(),
        });

        let sig = Signature {
            r: U256::from_be_bytes(hex!(
                "2a378831cf81d99a3f06a18ae1b6ca366817ab4d88a70053c41d7a8f0368e031"
            )),
            s: U256::from_be_bytes(hex!(
                "450d831a05b6e418724436c05c155e0a1b7b921015d0fbc2f667aed709ac4fb5"
            )),
            odd_y_parity: false,
        };

        let signed_tx = TransactionSigned::from_transaction_and_signature(tx, sig);
        assert_eq!(signed_tx.hash(), hash, "Expected same hash");
        assert_eq!(signed_tx.recover_signer(), Some(signer), "Recovering signer should pass.");
    }
}
