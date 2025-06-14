use crate::{
    types::{RollkitPayloadAttributes, PayloadAttributesError},
};
use alloy_primitives::{Address, B256};
use reth_primitives::TransactionSigned;

#[cfg(test)]
mod tests {
    use super::*;

    /// Test payload attributes creation and validation
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

    /// Test payload attributes validation
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
        assert!(attrs.validate().is_ok());

        // Test valid attributes with no gas limit
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            None,
            1234567890,
            B256::random(),
            Address::random(),
            B256::random(),
            1,
        );
        assert!(attrs.validate().is_ok());

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
        assert!(attrs.validate().is_err());
    }

    /// Test payload attributes serialization
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

    /// Test error types
    #[test]
    fn test_payload_attributes_errors() {
        // Test that error types can be created and formatted
        let error = PayloadAttributesError::EmptyTransactions;
        assert_eq!(error.to_string(), "No transactions provided");

        let error = PayloadAttributesError::InvalidGasLimit;
        assert_eq!(error.to_string(), "Invalid gas limit");

        let error = PayloadAttributesError::TransactionValidation("test error".to_string());
        assert_eq!(error.to_string(), "Transaction validation failed: test error");
    }

    /// Test payload attributes with different values
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
        assert!(attrs.validate().is_ok());

        // Test with zero values (except gas limit)
        let attrs = RollkitPayloadAttributes::new(
            vec![],
            Some(1),
            0,
            B256::ZERO,
            Address::ZERO,
            B256::ZERO,
            0,
        );
        assert!(attrs.validate().is_ok());
    }
} 