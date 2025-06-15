use crate::{
    types::{RollkitPayloadAttributes, PayloadAttributesError},
};
use alloy_primitives::{Address, B256};

#[cfg(test)]
mod tests {
    use super::*;

    /// Test payload attributes creation and basic field assignment
    #[test]
    fn test_payload_attributes_creation() {
        let transactions = vec![];
        let gas_limit = Some(1000000u64);
        let timestamp = 1234567890u64;
        let prev_randao = B256::random();
        let suggested_fee_recipient = Address::random();
        let parent_hash = B256::random();
        let block_number = 1u64;

        let attrs = RollkitPayloadAttributes::new(
            transactions.clone(),
            gas_limit,
            timestamp,
            prev_randao,
            suggested_fee_recipient,
            parent_hash,
            block_number,
        );

        assert_eq!(attrs.transactions, transactions);
        assert_eq!(attrs.gas_limit, gas_limit);
        assert_eq!(attrs.timestamp, timestamp);
        assert_eq!(attrs.prev_randao, prev_randao);
        assert_eq!(attrs.suggested_fee_recipient, suggested_fee_recipient);
        assert_eq!(attrs.parent_hash, parent_hash);
        assert_eq!(attrs.block_number, block_number);
    }

    /// Test comprehensive payload attributes validation including gas limits
    #[test]
    fn test_payload_attributes_validation() {
        // Test valid attributes with empty transactions (allowed for rollkit)
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(1000000),
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );
        assert!(attrs.validate().is_ok(), "Valid attributes should pass validation");

        // Test valid attributes with no gas limit (should use default)
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            None,
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );
        assert!(attrs.validate().is_ok(), "No gas limit should be valid (use default)");

        // Test invalid attributes with zero gas limit
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(0),
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );
        assert!(attrs.validate().is_err(), "Zero gas limit should be invalid");
        assert!(matches!(attrs.validate().unwrap_err(), PayloadAttributesError::InvalidGasLimit));
    }

    /// Test gas limit validation edge cases  
    #[test]
    fn test_gas_limit_validation() {
        // Test minimum valid gas limit (1)
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(1),
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );
        assert!(attrs.validate().is_ok(), "Minimum gas limit (1) should be valid");

        // Test maximum gas limit
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(u64::MAX),
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );
        assert!(attrs.validate().is_ok(), "Maximum gas limit should be valid");

        // Test typical gas limits
        for gas_limit in [21_000, 1_000_000, 30_000_000, 100_000_000] {
            let attrs = RollkitPayloadAttributes::new(
                vec![],
                Some(gas_limit),
                1234567890,
                B256::random(),
                Address::random(),
                B256::random(),
                1,
            );
            assert!(attrs.validate().is_ok(), "Typical gas limit {} should be valid", gas_limit);
        }
    }

    /// Test payload attributes serialization and deserialization
    #[test]
    fn test_payload_attributes_serialization() {
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(1000000),
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );

        let serialized = serde_json::to_string(&attrs).unwrap();
        let deserialized: RollkitPayloadAttributes = serde_json::from_str(&serialized).unwrap();

        assert_eq!(attrs.transactions.len(), deserialized.transactions.len());
        assert_eq!(attrs.gas_limit, deserialized.gas_limit);
        assert_eq!(attrs.timestamp, deserialized.timestamp);
        assert_eq!(attrs.prev_randao, deserialized.prev_randao);
        assert_eq!(attrs.suggested_fee_recipient, deserialized.suggested_fee_recipient);
        assert_eq!(attrs.parent_hash, deserialized.parent_hash);
        assert_eq!(attrs.block_number, deserialized.block_number);
    }

    /// Test all error types and their string representations
    #[test]
    fn test_payload_attributes_errors() {
        // Test that error types can be created and formatted correctly
        let error = PayloadAttributesError::EmptyTransactions;
        assert_eq!(error.to_string(), "No transactions provided");

        let error = PayloadAttributesError::InvalidGasLimit;
        assert_eq!(error.to_string(), "Invalid gas limit");

        let error = PayloadAttributesError::TransactionValidation("test error".to_string());
        assert_eq!(error.to_string(), "Transaction validation failed: test error");
    }

    /// Test payload attributes with edge case values
    #[test]
    fn test_payload_attributes_edge_cases() {
        // Test with maximum timestamp
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(u64::MAX),
            u64::MAX,
            B256::repeat_byte(0xFF),
            Address::repeat_byte(0xFF),
            B256::repeat_byte(0xFF),
            u64::MAX,
        );
        assert!(attrs.validate().is_ok(), "Maximum values should be valid");

        // Test with zero values (except gas limit which must be > 0)
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(1), // Minimum valid gas limit
            0,
            B256::ZERO,
            Address::ZERO,
            B256::ZERO,
            0,
        );
        assert!(attrs.validate().is_ok(), "Zero values (except gas limit) should be valid");

        // Test with empty byte values
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            None, // No gas limit specified
            1234567890,
            B256::ZERO,
            Address::ZERO,
            B256::ZERO,
            1,
        );
        assert!(attrs.validate().is_ok(), "Zero byte values should be valid");
    }

    /// Test validation consistency across different scenarios
    #[test]
    fn test_validation_consistency() {
        // Test that validation is consistent regardless of other field values
        let base_attrs = |gas_limit| RollkitPayloadAttributes::new(
            vec![],
            gas_limit,
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );

        // Valid gas limits should always pass
        assert!(base_attrs(Some(1)).validate().is_ok());
        assert!(base_attrs(Some(21_000)).validate().is_ok());
        assert!(base_attrs(Some(30_000_000)).validate().is_ok());
        assert!(base_attrs(None).validate().is_ok());

        // Invalid gas limits should always fail
        assert!(base_attrs(Some(0)).validate().is_err());
    }
} 