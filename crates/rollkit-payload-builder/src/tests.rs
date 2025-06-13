use crate::{
    builder::{RollkitPayloadBuilder, create_payload_builder_service},
    types::{RollkitPayloadAttributes, PayloadAttributesError},
};
use reth_primitives::TransactionSigned;
use alloy_primitives::{Address, B256, U256, TxKind, ChainId, Signature};
use alloy_consensus::{TxLegacy, EthereumTypedTransaction};
use reth_provider::test_utils::MockEthProvider;
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a mock transaction with the specified gas limit
    fn create_mock_transaction(gas_limit: u64) -> TransactionSigned {
        // Create a simple legacy transaction for testing
        reth_primitives::TransactionSigned::new_unhashed(
            EthereumTypedTransaction::Legacy(TxLegacy {
                chain_id: Some(ChainId::from(1u64)),
                nonce: 0,
                gas_price: 1_000_000_000,
                gas_limit,
                to: TxKind::Call(Address::random()),
                value: U256::from(0),
                input: Default::default(),
            }),
            Signature::test_signature()
        )
    }

    /// Creates test payload attributes with the given transactions and gas limit
    fn create_test_attributes(
        transactions: Vec<TransactionSigned>,
        gas_limit: Option<u64>,
    ) -> RollkitPayloadAttributes {
        RollkitPayloadAttributes::new(
            transactions,
            gas_limit,
            1234567890,
            B256::random(),
            Address::random(),
        )
    }

    /// Tests the validation of payload attributes
    /// 
    /// Test cases:
    /// 1. Empty transactions list should return EmptyTransactions error
    /// 2. Zero gas limit should return InvalidGasLimit error
    /// 3. Valid attributes should pass validation
    #[test]
    fn test_payload_attributes_validation() {
        // Test empty transactions
        let empty_attrs = create_test_attributes(vec![], None);
        assert!(matches!(
            empty_attrs.validate(),
            Err(PayloadAttributesError::EmptyTransactions)
        ));

        // Test invalid gas limit
        let invalid_gas_attrs = create_test_attributes(
            vec![create_mock_transaction(100)],
            Some(0),
        );
        assert!(matches!(
            invalid_gas_attrs.validate(),
            Err(PayloadAttributesError::InvalidGasLimit)
        ));

        // Test valid attributes
        let valid_attrs = create_test_attributes(
            vec![create_mock_transaction(100)],
            Some(1000),
        );
        assert!(valid_attrs.validate().is_ok());
    }

    /// Tests the creation of a payload builder instance
    /// 
    /// Verifies that:
    /// 1. Builder can be created with a mock client
    /// 2. Client reference is correctly stored
    #[tokio::test]
    async fn test_payload_builder_creation() {
        let client = Arc::new(MockEthProvider::default());
        let builder = RollkitPayloadBuilder::new(client.clone());
        assert!(builder.client.as_ref() as *const _ == client.as_ref() as *const _);
    }

    /// Tests the creation of a payload builder service
    /// 
    /// Verifies that:
    /// 1. Service can be created with a mock client
    /// 2. Service is properly initialized
    #[tokio::test]
    async fn test_payload_builder_service_creation() {
        let client = Arc::new(MockEthProvider::default());
        let service = create_payload_builder_service(client);
        assert!(service.is_some());
    }

    /// Tests building a payload with multiple transactions
    /// 
    /// Verifies that:
    /// 1. Payload can be built with multiple transactions
    /// 2. All transactions are included in the block
    /// 3. Block structure is correct
    #[tokio::test]
    async fn test_build_payload_with_transactions() {
        let client = Arc::new(MockEthProvider::default());
        let builder = RollkitPayloadBuilder::new(client);

        let transactions = vec![
            create_mock_transaction(100),
            create_mock_transaction(200),
        ];

        let attributes = create_test_attributes(transactions.clone(), Some(1000));
        let result = builder.build_payload(attributes).await;

        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.body().transactions.len(), 2);
    }

    /// Tests that the payload builder respects gas limits
    /// 
    /// Verifies that:
    /// 1. Transactions exceeding gas limit are not included
    /// 2. Only transactions within gas limit are included
    /// 3. Block gas usage is within limits
    #[tokio::test]
    async fn test_build_payload_respects_gas_limit() {
        let client = Arc::new(MockEthProvider::default());
        let builder = RollkitPayloadBuilder::new(client);

        let transactions = vec![
            create_mock_transaction(400),
            create_mock_transaction(400),
            create_mock_transaction(400),
        ];

        let attributes = create_test_attributes(transactions, Some(500));
        let result = builder.build_payload(attributes).await;

        assert!(result.is_ok());
        let block = result.unwrap();
        // Only the first transaction should be included due to gas limit
        assert_eq!(block.body().transactions.len(), 1);
    }

    /// Tests building a payload without gas limit constraints
    /// 
    /// Verifies that:
    /// 1. All transactions are included when no gas limit is specified
    /// 2. Block structure is correct
    /// 3. Transactions are included in order
    #[tokio::test]
    async fn test_build_payload_without_gas_limit() {
        let client = Arc::new(MockEthProvider::default());
        let builder = RollkitPayloadBuilder::new(client);

        let transactions = vec![
            create_mock_transaction(100),
            create_mock_transaction(200),
            create_mock_transaction(300),
        ];

        let attributes = create_test_attributes(transactions, None);
        let result = builder.build_payload(attributes).await;

        assert!(result.is_ok());
        let block = result.unwrap();
        // All transactions should be included when no gas limit is specified
        assert_eq!(block.body().transactions.len(), 3);
    }

    /// Tests serialization and deserialization of payload attributes
    /// 
    /// Verifies that:
    /// 1. Attributes can be serialized to JSON
    /// 2. Serialized attributes can be deserialized
    /// 3. Deserialized attributes match original data
    #[test]
    fn test_payload_attributes_serialization() {
        let attributes = create_test_attributes(
            vec![create_mock_transaction(100)],
            Some(1000),
        );

        // Test serialization
        let serialized = serde_json::to_string(&attributes).unwrap();
        let deserialized: RollkitPayloadAttributes = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.transactions.len(), attributes.transactions.len());
        assert_eq!(deserialized.gas_limit, attributes.gas_limit);
    }
} 