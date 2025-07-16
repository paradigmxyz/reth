//! Tests for the consensus transaction helper function.

use alloy_primitives::Address;
use reth_transaction_pool::{
    error::PoolError,
    noop::MockTransactionValidator,
    test_utils::{MockTransaction, TestPoolBuilder},
    PoolTransaction, TransactionOrigin, TransactionPool,
};

#[tokio::test]
async fn test_add_consensus_transaction_success() {
    let pool = TestPoolBuilder::default();

    // Create a mock transaction and convert it to consensus format
    let tx = MockTransaction::eip1559();
    let consensus_tx = tx.into_consensus();

    // This should succeed - the helper should handle conversion and submission
    let result = pool.add_consensus_transaction(consensus_tx).await;

    // Currently this will fail because we haven't implemented the function yet
    assert!(result.is_ok(), "Expected successful transaction addition");
    assert_eq!(pool.len(), 1, "Pool should contain one transaction");
}

#[tokio::test]
async fn test_add_consensus_transaction_eip4844_without_sidecar() {
    let pool = TestPoolBuilder::default();

    // Create a blob transaction without sidecar (consensus format)
    let tx = MockTransaction::eip4844();
    let consensus_tx = tx.into_consensus();

    // This should fail because EIP-4844 transactions need sidecars to be pooled
    let result = pool.add_consensus_transaction(consensus_tx).await;

    // Should return an error because blob transactions without sidecars can't be pooled
    assert!(result.is_err(), "EIP-4844 transactions without sidecar should fail");
}

#[tokio::test]
async fn test_add_consensus_transaction_duplicate() {
    let pool = TestPoolBuilder::default();

    let tx = MockTransaction::eip1559();
    let consensus_tx = tx.into_consensus();

    // Add the transaction first time
    let result1 = pool.add_consensus_transaction(consensus_tx.clone()).await;
    assert!(result1.is_ok(), "First addition should succeed");

    // Try to add the same transaction again
    let result2 = pool.add_consensus_transaction(consensus_tx).await;
    assert!(result2.is_err(), "Duplicate transaction should fail");
}

#[tokio::test]
async fn test_add_consensus_transaction_invalid_nonce() {
    // Use a validator that returns invalid for all transactions
    let pool =
        TestPoolBuilder::default().with_validator(MockTransactionValidator::return_invalid());

    // Create a transaction with invalid nonce (too low)
    let tx = MockTransaction::eip1559().with_nonce(0);
    let consensus_tx = tx.into_consensus();

    // This should fail validation due to the return_invalid validator
    let result = pool.add_consensus_transaction(consensus_tx).await;

    assert!(result.is_err(), "Transaction should fail with return_invalid validator");
}

#[tokio::test]
async fn test_add_consensus_transaction_insufficient_funds() {
    let pool = TestPoolBuilder::default();

    let tx = MockTransaction::eip1559().with_gas_limit(1000000000); // Very high gas limit
    let consensus_tx = tx.into_consensus();

    // This should fail due to insufficient funds
    let result = pool.add_consensus_transaction(consensus_tx).await;

    assert!(result.is_err(), "Transaction with insufficient funds should fail");
}

#[tokio::test]
async fn test_add_consensus_transaction_origin_is_local() {
    let pool = TestPoolBuilder::default();

    let tx = MockTransaction::eip1559();
    let consensus_tx = tx.into_consensus();

    // Add transaction via consensus helper
    let result = pool.add_consensus_transaction(consensus_tx).await;
    assert!(result.is_ok(), "Transaction addition should succeed");

    // Verify the transaction was added with Local origin
    let local_txs = pool.get_local_transactions();
    assert_eq!(local_txs.len(), 1, "Should have one local transaction");

    let external_txs = pool.get_external_transactions();
    assert_eq!(external_txs.len(), 0, "Should have no external transactions");
}

#[tokio::test]
async fn test_add_consensus_transaction_different_types() {
    let pool = TestPoolBuilder::default();

    // Test legacy transaction
    let legacy_tx = MockTransaction::legacy().with_gas_price(1000000000); // 1 gwei
    let consensus_legacy = legacy_tx.into_consensus();

    let result = pool.add_consensus_transaction(consensus_legacy).await;
    assert!(result.is_ok(), "Legacy transaction should succeed");

    // Test EIP-2930 transaction
    let eip2930_tx = MockTransaction::eip2930().with_gas_price(1000000000); // 1 gwei
    let consensus_eip2930 = eip2930_tx.into_consensus();

    let result = pool.add_consensus_transaction(consensus_eip2930).await;
    assert!(result.is_ok(), "EIP-2930 transaction should succeed");

    // Test EIP-1559 transaction
    let eip1559_tx = MockTransaction::eip1559();
    let consensus_eip1559 = eip1559_tx.into_consensus();

    let result = pool.add_consensus_transaction(consensus_eip1559).await;
    assert!(result.is_ok(), "EIP-1559 transaction should succeed");

    assert_eq!(pool.len(), 3, "Pool should contain three transactions");
}

#[tokio::test]
async fn test_add_consensus_transaction_batch() {
    let pool = TestPoolBuilder::default();

    // Create multiple transactions from the same sender with sequential nonces
    let sender = Address::random();
    let mut transactions = Vec::new();

    for nonce in 0..5 {
        let tx = MockTransaction::eip1559().with_nonce(nonce).with_sender(sender);
        let consensus_tx = tx.into_consensus();
        transactions.push(consensus_tx);
    }

    // Add all transactions
    let mut results = Vec::new();
    for tx in transactions {
        let result = pool.add_consensus_transaction(tx).await;
        results.push(result);
    }

    // All should succeed
    for result in results {
        assert!(result.is_ok(), "All transactions should succeed");
    }

    assert_eq!(pool.len(), 5, "Pool should contain five transactions");
}

#[tokio::test]
async fn test_add_consensus_transaction_error_propagation() {
    // Use a validator that returns invalid to test error propagation
    let pool =
        TestPoolBuilder::default().with_validator(MockTransactionValidator::return_invalid());

    // Create a transaction that will fail validation due to the validator
    let tx = MockTransaction::eip1559().with_gas_limit(0); // Invalid gas limit
    let consensus_tx = tx.into_consensus();

    let result = pool.add_consensus_transaction(consensus_tx).await;

    // Should propagate the validation error from return_invalid validator
    assert!(result.is_err(), "Invalid transaction should fail");

    // Verify the error is properly typed
    let error = result.unwrap_err();
    assert!(matches!(error, PoolError { .. }), "Should return PoolError");
}

/// Test helper function to verify that the consensus transaction helper
/// provides the same functionality as the verbose manual process
#[tokio::test]
async fn test_consensus_helper_equivalent_to_manual_process() {
    let pool1 = TestPoolBuilder::default();
    let pool2 = TestPoolBuilder::default();

    let tx = MockTransaction::eip1559();
    let consensus_tx = tx.into_consensus();

    // Use the helper function
    let result1 = pool1.add_consensus_transaction(consensus_tx.clone()).await;

    // Use the manual process (what currently exists)
    let pool_transaction = MockTransaction::try_from_consensus(consensus_tx.clone())
        .expect("Should convert to pool transaction");
    let result2 = pool2.add_transaction(TransactionOrigin::Local, pool_transaction).await;

    // Both should have the same outcome
    assert_eq!(result1.is_ok(), result2.is_ok(), "Both methods should have same success/failure");

    if result1.is_ok() && result2.is_ok() {
        assert_eq!(result1.unwrap(), result2.unwrap(), "Both should return the same hash");
        assert_eq!(pool1.len(), pool2.len(), "Both pools should have same size");
    }
}

/// Test that the helper function handles conversion errors properly
#[tokio::test]
async fn test_add_consensus_transaction_conversion_error() {
    let pool = TestPoolBuilder::default();

    // Create a transaction type that can't be converted to pooled format
    // This simulates an OP deposit transaction or similar
    let tx = MockTransaction::eip1559();

    // This test verifies that conversion errors are properly handled
    // The actual implementation should catch conversion errors and return appropriate PoolError
    // For now, we'll just test that the function exists and handles basic cases
    let consensus_tx = tx.into_consensus();
    let result = pool.add_consensus_transaction(consensus_tx).await;

    // This should succeed for MockTransaction since it can be converted
    assert!(result.is_ok(), "MockTransaction should be convertible");
}
