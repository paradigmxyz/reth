//! Payload builder tests for the Rollkit payload builder.
//!
//! This test suite focuses on the core payload building functionality,
//! including empty payloads, payloads with transactions, error handling,
//! and performance metrics.

mod common;

use std::time::Duration;
use tokio::time::timeout;
use eyre::Result;

use common::{RollkitTestFixture, create_test_transactions, TEST_TIMESTAMP, TEST_GAS_LIMIT};

/// Tests basic payload building with empty transactions
#[tokio::test]
async fn test_empty_payload_building() -> Result<()> {
    let fixture = RollkitTestFixture::new().await?;
    
    let payload_attrs = fixture.create_payload_attributes(
        vec![], // Empty transactions
        1,
        TEST_TIMESTAMP,
        fixture.genesis_hash,
        Some(TEST_GAS_LIMIT),
    );
    
    let result = fixture.builder.build_payload(payload_attrs).await;
    assert!(result.is_ok(), "Empty payload should build successfully");
    
    let sealed_block = result.unwrap();
    assert_eq!(sealed_block.transaction_count(), 0, "Empty payload should have 0 transactions");
    assert_eq!(sealed_block.number, 1, "Block number should be 1");
    
    println!("✓ Empty payload building test passed");
    Ok(())
}

/// Tests payload building with multiple transactions
#[tokio::test]
async fn test_payload_with_transactions() -> Result<()> {
    let fixture = RollkitTestFixture::new().await?;
    
    // Create test transactions
    let transactions = create_test_transactions(3, 0);
    let payload_attrs = fixture.create_payload_attributes(
        transactions,
        1,
        TEST_TIMESTAMP,
        fixture.genesis_hash,
        Some(TEST_GAS_LIMIT),
    );
    
    let result = timeout(
        Duration::from_secs(30),
        fixture.builder.build_payload(payload_attrs)
    ).await;
    
    match result {
        Ok(Ok(sealed_block)) => {
            assert_eq!(sealed_block.number, 1, "Block number should be 1");
            assert!(sealed_block.transaction_count() >= 1, "Block should contain transactions");
            println!("✓ Payload with transactions test passed: {} transactions", sealed_block.transaction_count());
        }
        Ok(Err(e)) => {
            panic!("Payload building failed: {:?}", e);
        }
        Err(_) => {
            panic!("Payload building timed out");
        }
    }
    
    Ok(())
}

/// Tests payload building with various transaction counts
#[tokio::test]
async fn test_variable_transaction_counts() -> Result<()> {
    let fixture = RollkitTestFixture::new().await?;
    
    // Test different transaction counts
    for tx_count in [0, 1, 5, 10, 50] {
        let transactions = if tx_count > 0 {
            create_test_transactions(tx_count, 0)
        } else {
            vec![]
        };
        
        let payload_attrs = fixture.create_payload_attributes(
            transactions,
            1,
            TEST_TIMESTAMP,
            fixture.genesis_hash,
            Some(TEST_GAS_LIMIT),
        );
        
        let result = fixture.builder.build_payload(payload_attrs).await;
        assert!(result.is_ok(), "Payload with {} transactions should build successfully", tx_count);
        
        let sealed_block = result.unwrap();
        if tx_count == 0 {
            assert_eq!(sealed_block.transaction_count(), 0, "Empty block should have 0 transactions");
        } else {
            assert!(sealed_block.transaction_count() >= 1, "Non-empty block should have transactions");
        }
        
        println!("✓ Built payload with {} transactions", tx_count);
    }
    
    println!("✓ Variable transaction counts test passed");
    Ok(())
}

/// Tests error handling for invalid payload building scenarios
#[tokio::test]
async fn test_payload_building_error_handling() -> Result<()> {
    let fixture = RollkitTestFixture::new().await?;
    
    // Test with invalid gas limit (should fail during validation, not building)
    let invalid_attrs = fixture.create_payload_attributes(
        vec![],
        1,
        TEST_TIMESTAMP,
        fixture.genesis_hash,
        Some(0), // Invalid gas limit
    );
    
    // This should fail during validation, not during building
    assert!(invalid_attrs.validate().is_err(), "Invalid gas limit should fail validation");
    
    // Test with extremely large transaction count
    let large_tx_count = 1000;
    let transactions = create_test_transactions(large_tx_count, 0);
    let payload_attrs = fixture.create_payload_attributes(
        transactions,
        1,
        TEST_TIMESTAMP,
        fixture.genesis_hash,
        Some(TEST_GAS_LIMIT),
    );
    
    let result = fixture.builder.build_payload(payload_attrs).await;
    match result {
        Ok(sealed_block) => {
            println!("✓ Large transaction batch handled: {} transactions, gas_used={}", 
                large_tx_count, sealed_block.gas_used);
        }
        Err(e) => {
            println!("✓ Large transaction batch rejected appropriately: {}", e);
        }
    }
    
    // Test with very large timestamp (should still work)
    let attrs_large_timestamp = fixture.create_payload_attributes(
        vec![],
        1,
        u64::MAX, // Very large timestamp
        fixture.genesis_hash,
        Some(TEST_GAS_LIMIT),
    );
    
    let result = fixture.builder.build_payload(attrs_large_timestamp).await;
    match result {
        Ok(_) => println!("✓ Large timestamp handled gracefully"),
        Err(e) => println!("✓ Large timestamp rejected appropriately: {}", e),
    }
    
    println!("✓ Error handling tests completed");
    Ok(())
}

/// Tests performance with different payload sizes
#[tokio::test]
async fn test_payload_building_performance() -> Result<()> {
    let fixture = RollkitTestFixture::new().await?;
    
    // Test different transaction batch sizes
    let batch_sizes = vec![1, 10, 50, 100];
    
    for batch_size in batch_sizes {
        let transactions = create_test_transactions(batch_size, 0);
        let payload_attrs = fixture.create_payload_attributes(
            transactions,
            1,
            TEST_TIMESTAMP,
            fixture.genesis_hash,
            Some(TEST_GAS_LIMIT),
        );
        
        let start_time = std::time::Instant::now();
        let result = fixture.builder.build_payload(payload_attrs).await;
        let duration = start_time.elapsed();
        
        match result {
            Ok(sealed_block) => {
                println!("✓ Batch size {}: {:?}, gas_used={}", 
                    batch_size, duration, sealed_block.gas_used);
                
                // Basic performance assertions
                assert!(duration < Duration::from_secs(30), 
                    "Payload building should complete within 30 seconds");
                assert_eq!(sealed_block.number, 1, "Block number should be correct");
            }
            Err(e) => {
                println!("✗ Batch size {} failed: {:?}", batch_size, e);
                return Err(e.into());
            }
        }
    }
    
    println!("✓ Performance tests completed");
    Ok(())
}

/// Tests payload building with different gas limits
#[tokio::test]
async fn test_gas_limit_scenarios() -> Result<()> {
    let fixture = RollkitTestFixture::new().await?;
    
    let transactions = create_test_transactions(2, 0);
    
    // Test with no gas limit (should use default)
    let payload_attrs = fixture.create_payload_attributes(
        transactions.clone(),
        1,
        TEST_TIMESTAMP,
        fixture.genesis_hash,
        None, // No gas limit
    );
    
    let result = fixture.builder.build_payload(payload_attrs).await;
    assert!(result.is_ok(), "Payload with no gas limit should build successfully");
    
    // Test with specific gas limits
    let gas_limits = vec![100_000, 1_000_000, 30_000_000];
    
    for gas_limit in gas_limits {
        let payload_attrs = fixture.create_payload_attributes(
            transactions.clone(),
            1,
            TEST_TIMESTAMP,
            fixture.genesis_hash,
            Some(gas_limit),
        );
        
        let result = fixture.builder.build_payload(payload_attrs).await;
        assert!(result.is_ok(), "Payload with gas limit {} should build successfully", gas_limit);
        
        let sealed_block = result.unwrap();
        println!("✓ Gas limit {}: gas_used={}", gas_limit, sealed_block.gas_used);
    }
    
    println!("✓ Gas limit scenarios test passed");
    Ok(())
} 