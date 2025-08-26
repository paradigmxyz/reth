#[cfg(test)]
mod debug_tests {
    use crate::Receipt;
    use alloy_primitives::{Address, Bytes, Log, TxType, B256};
    use crate::TxType;


    struct DebugReceipt<'a>(&'a Receipt);

    impl<'a> std::fmt::Debug for DebugReceipt<'a> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Receipt")
                .field("success", &self.0.success)
                .field("cumulative_gas_used", &self.0.cumulative_gas_used)
                .field("logs_count", &self.0.logs.len())
                .field("tx_type", &self.0.tx_type)
                .finish()
        }
    }

    #[test]
    fn test_receipt_debug_format() {
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 21000,
            logs: Vec::new(),
        };

        let debug_output = format!("{:?}", DebugReceipt(&receipt));

        assert!(debug_output.contains("Receipt"));
        assert!(debug_output.contains("success: true"));
        assert!(debug_output.contains("cumulative_gas_used: 21000"));
        assert!(debug_output.contains("logs_count: 0"));
        assert!(debug_output.contains("tx_type:"));
    }
}
