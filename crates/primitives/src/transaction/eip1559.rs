pub use alloy_consensus::TxEip1559;
#[cfg(test)]
mod tests {
    use super::TxEip1559;
    use crate::{
        transaction::{signature::Signature, TxKind},
        AccessList, Address, Transaction, TransactionSigned, B256, U256,
    };
    use std::str::FromStr;

    #[test]
    fn recover_signer_eip1559() {
        use crate::hex_literal::hex;

        let signer: Address = hex!("dd6b8b3dc6b7ad97db52f08a275ff4483e024cea").into();
        let hash: B256 =
            hex!("0ec0b6a2df4d87424e5f6ad2a654e27aaeb7dac20ae9e8385cc09087ad532ee0").into();

        let tx = Transaction::Eip1559( TxEip1559 {
            chain_id: 1,
            nonce: 0x42,
            gas_limit: 44386,
            to: TxKind::Call(hex!("6069a6c32cf691f5982febae4faf8a6f3ab2f0f6").into()),
            value: U256::ZERO,
            input:  hex!("a22cb4650000000000000000000000005eee75727d804a2b13038928d36f8b188945a57a0000000000000000000000000000000000000000000000000000000000000000").into(),
            max_fee_per_gas: 0x4a817c800,
            max_priority_fee_per_gas: 0x3b9aca00,
            access_list: AccessList::default(),
        });

        let sig = Signature {
            r: U256::from_str("0x840cfc572845f5786e702984c2a582528cad4b49b2a10b9db1be7fca90058565")
                .unwrap(),
            s: U256::from_str("0x25e7109ceb98168d95b09b18bbf6b685130e0562f233877d492b94eee0c5b6d1")
                .unwrap(),
            odd_y_parity: false,
        };

        let signed_tx = TransactionSigned::from_transaction_and_signature(tx, sig);
        assert_eq!(signed_tx.hash(), hash, "Expected same hash");
        assert_eq!(signed_tx.recover_signer(), Some(signer), "Recovering signer should pass.");
    }
}
