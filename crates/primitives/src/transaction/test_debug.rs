#[cfg(test)]
mod debug_tests {
    use crate::{TransactionSigned};
    use alloy_consensus::{Signed, Transaction};
    use alloy_consensus::{TxLegacy, TxEip1559, TxKind};

    use alloy_primitives::{Address, Bytes, Signature, TxEip1559, TxKind, TxLegacy, B256, U256};

    struct DebugTransactionSigned<'a>(&'a TransactionSigned);

    impl<'a> std::fmt::Debug for DebugTransactionSigned<'a> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self.0 {
                TransactionSigned::Legacy(ref tx) => f
                    .debug_struct("TransactionSigned::Legacy")
                    .field("hash", &tx.hash())
                    .field("nonce", &tx.nonce())
                    .field("gas_limit", &tx.gas_limit())
                    .finish(),
                TransactionSigned::Eip2930(ref tx) => f
                    .debug_struct("TransactionSigned::Eip2930")
                    .field("hash", &tx.hash())
                    .field("nonce", &tx.nonce())
                    .field("gas_limit", &tx.gas_limit())
                    .finish(),
                TransactionSigned::Eip1559(ref tx) => f
                    .debug_struct("TransactionSigned::Eip1559")
                    .field("hash", &tx.hash())
                    .field("nonce", &tx.nonce())
                    .field("gas_limit", &tx.gas_limit())
                    .finish(),
                TransactionSigned::Eip4844(ref tx) => f
                    .debug_struct("TransactionSigned::Eip4844")
                    .field("hash", &tx.hash())
                    .field("nonce", &tx.nonce())
                    .field("gas_limit", &tx.gas_limit())
                    .finish(),
                TransactionSigned::Eip7702(ref tx) => f
                    .debug_struct("TransactionSigned::Eip7702")
                    .field("hash", &tx.hash())
                    .field("nonce", &tx.nonce())
                    .field("gas_limit", &tx.gas_limit())
                    .finish(),
            }
        }
    }

    #[test]
    fn test_transaction_signed_debug_format() {
        let legacy_tx = TxLegacy {
            nonce: 42,
            gas_price: U256::from(20_000_000_000u64),
            gas_limit: U256::from(21000u64),
            to: TxKind::Call(Address::random()),
            value: U256::from(1_000_000_000_000_000_000u64),
            input: Bytes::default(),
        };

        let signature =
            Signature::from_scalars_and_parity(U256::from(1), U256::from(1), false).unwrap();

        let signed_legacy = Signed::new_unchecked(legacy_tx, signature, B256::random());
        let tx_signed = TransactionSigned::Legacy(signed_legacy);

        let debug_output = format!("{:?}", DebugTransactionSigned(&tx_signed));
        assert!(debug_output.contains("TransactionSigned::Legacy"));
        assert!(debug_output.contains("nonce: 42"));
        assert!(debug_output.contains("gas_limit: 21000"));
        assert!(debug_output.contains("hash:"));
    }
}
