#[cfg(test)]
mod debug_tests {
    use crate::Header;
    use alloy_consensus::Hash;
    use alloy_primitives::{Address, Bloom, B256, U256};

    struct DebugHeader<'a>(&'a Header);

    impl<'a> std::fmt::Debug for DebugHeader<'a> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Header")
                .field("number", &self.0.number)
                .field("hash", &self.0.hash())
                .field("parent_hash", &self.0.parent_hash)
                .field("timestamp", &self.0.timestamp)
                .field("gas_used", &self.0.gas_used)
                .field("gas_limit", &self.0.gas_limit)
                .finish()
        }
    }

    #[test]
    fn test_header_debug_format() {
        let header = Header {
            number: 12345,
            gas_limit: U256::from(8_000_000u64),
            gas_used: U256::from(7_500_000u64),
            timestamp: 1625097600,
            parent_hash: B256::random(),
            ..Default::default()
        };

        let debug_output = format!("{:?}", DebugHeader(&header));

        assert!(debug_output.contains("Header"));
        assert!(debug_output.contains("number: 12345"));
        assert!(debug_output.contains("timestamp: 1625097600"));
        assert!(debug_output.contains("gas_used: 7500000"));
        assert!(debug_output.contains("gas_limit: 8000000"));
        assert!(debug_output.contains("hash:"));
        assert!(debug_output.contains("parent_hash:"));
    }
}
